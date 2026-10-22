mod cache;
pub mod install;
pub mod queries;

use crate::config::config::read_config;
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use url::Url;

// Bedrock Game ID (CurseForge).
const GAME_ID: &str = "78022";
const CURSEFORGE_OFFICIAL_BASE: &str = "https://api.curseforge.com";
const CURSEFORGE_MIRROR_BASE: &str = "https://mod.mcimirror.top/curseforge";

// NOTE: This matches the upstream web/tauri implementation. Prefer configuring a proxy/base URL
// over changing this constant.
const API_KEY: &str = "$2a$10$3Dr/WLO28GST4n7h7vD0zeWNPjIbwqb1cyVsL66BXAfliCpBC5Ejm";
const CURSEFORGE_API_MAX_ATTEMPTS: usize = 3;
const CURSEFORGE_API_RETRY_DELAY_MS: u64 = 200;
const MIRROR_FAILOVER_COOLDOWN: Duration = Duration::from_secs(45);
const MIRROR_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

// Shared across short-lived clients so a failing mirror does not stall every page.
static MIRROR_UNHEALTHY_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Clone)]
pub struct CurseForgeClient {
    client: Client,
    base_v1: String,
    base_v2: String,
    mirror_failover: bool,
    api_key: String,
    custom_endpoint: bool,
}

impl CurseForgeClient {
    pub fn new() -> Result<Self, String> {
        let client = build_http_client()?.clone();
        let (base_v1, base_v2, mirror_failover) = build_base_urls()?;
        let auth_config = read_config()
            .map_err(|error| format!("读取 CurseForge API Key 配置失败：{error}"))?;
        let api_key = auth_config.launcher.download.curseforge_api_key;
        let custom_endpoint = auth_config.launcher.download.curseforge_api_source == "custom";
        Ok(Self {
            client,
            base_v1,
            base_v2,
            mirror_failover,
            api_key,
            custom_endpoint,
        })
    }

    fn credential_for(&self, url: &Url) -> Option<&str> {
        // Never send a personally supplied API key to third-party mirrors.
        // A custom HTTPS endpoint is explicitly chosen by the user.
        if url.scheme() != "https" {
            return None;
        }
        let host = url.host_str()?;
        if host == "api.curseforge.com" {
            return Some(if self.api_key.trim().is_empty() {
                API_KEY
            } else {
                self.api_key.trim()
            });
        }
        if self.custom_endpoint && !self.api_key.trim().is_empty() {
            return Some(self.api_key.trim());
        }
        if host == "mod.mcimirror.top" {
            return Some(API_KEY);
        }
        None
    }

    // Mirror selection is an automatic two-source policy: preserve the selected
    // mirror while it is healthy, and try the official API only for mirror errors.
    // Explicit official/custom sources never silently contact another host.
    async fn get<T>(&self, url: Url) -> Result<T, String>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let alternative = self.official_fallback_url(&url);
        if let Some(ref official) = alternative
            && mirror_in_cooldown()
        {
            return self
                .get_one::<T>(official.clone(), false)
                .await
                .map_err(|error| format!("CurseForge 镜像暂不可用，官方 API 请求失败：{}", error.message));
        }

        match self.get_one::<T>(url.clone(), alternative.is_some()).await {
            Ok(value) => Ok(value),
            Err(error) if error.fallback_allowed && alternative.is_some() => {
                if error.mirror_health_failure {
                    mirror_mark_unhealthy();
                }
                tracing::warn!(
                    mirror = %url,
                    reason = %error.message,
                    "CurseForge 镜像请求失败，自动回退官方 API"
                );
                self.get_one::<T>(alternative.expect("checked fallback presence"), false)
                    .await
                    .map_err(|official_error| format!(
                        "CurseForge 镜像失败（{}），官方 API 也失败（{}）",
                        error.message, official_error.message
                    ))
            }
            Err(error) => Err(error.message),
        }
    }

    fn official_fallback_url(&self, url: &Url) -> Option<Url> {
        if !self.mirror_failover {
            return None;
        }
        official_fallback_url(url)
    }

    // Each endpoint owns its own cache key. A successful official fallback must
    // never be stored as if it had come from the mirror.
    async fn get_one<T>(&self, url: Url, mirror_request: bool) -> Result<T, FetchError>
    where
        T: DeserializeOwned + Send + 'static,
    {
        // Keep cached results isolated when the user rotates an API key without
        // placing raw credentials in cache paths, logs, or URLs.
        let cache_key = format!("{}#auth={:016x}", url, auth_cache_tag(self.credential_for(&url)));
        let policy = cache::policy_for(&url);
        if let Some(body) = cache::load(&cache_key, policy.fresh_for).await {
            match serde_json::from_slice(&body) {
                Ok(value) => return Ok(value),
                Err(_) => cache::invalidate(&cache_key).await,
            }
        }

        if let Some(body) = cache::load(&cache_key, policy.stale_for).await {
            match serde_json::from_slice(&body) {
                Ok(value) => {
                    self.refresh_in_background::<T>(
                        url,
                        cache_key,
                        policy.fresh_for,
                        mirror_request,
                    );
                    return Ok(value);
                }
                Err(_) => cache::invalidate(&cache_key).await,
            }
        }

        let _request_guard = cache::request_guard(&cache_key).await;
        if let Some(body) = cache::load(&cache_key, policy.fresh_for).await {
            match serde_json::from_slice(&body) {
                Ok(value) => return Ok(value),
                Err(_) => cache::invalidate(&cache_key).await,
            }
        }

        // Another request may have tripped the breaker while we waited for
        // the per-URL cache lock. Avoid retrying the same broken mirror.
        if mirror_request && mirror_in_cooldown() {
            return Err(FetchError::cooldown());
        }
        let attempts = if mirror_request { 1 } else { CURSEFORGE_API_MAX_ATTEMPTS };
        match self.fetch(url, attempts, mirror_request).await {
            Ok(body) => {
                let parsed = serde_json::from_slice::<T>(&body)
                    .map_err(|error| FetchError::fallbackable(format!(
                        "CurseForge API 响应格式无效：{error}"
                    )))?;
                if mirror_request {
                    mirror_mark_healthy();
                }
                if let Err(error) = cache::store(&cache_key, Arc::<[u8]>::from(body)).await {
                    tracing::debug!("store CurseForge response cache failed: {error}");
                }
                Ok(parsed)
            }
            Err(error) if error.retryable && !mirror_request => {
                if let Some(body) = cache::load(&cache_key, policy.stale_for).await
                    && let Ok(value) = serde_json::from_slice(&body)
                {
                    tracing::warn!(
                        url = %cache_key,
                        reason = %error.message,
                        "CurseForge 请求失败，使用旧缓存"
                    );
                    return Ok(value);
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn refresh_in_background<T>(
        &self,
        url: Url,
        cache_key: String,
        fresh_for: Duration,
        mirror_request: bool,
    ) where
        T: DeserializeOwned + Send + 'static,
    {
        let client = self.clone();
        let refresh = async move {
            let _request_guard = cache::request_guard(&cache_key).await;
            if cache::load(&cache_key, fresh_for).await.is_some() {
                return;
            }

            if mirror_request && mirror_in_cooldown() {
                if let Some(official) = client.official_fallback_url(&url) {
                    let _ = client.get_one::<T>(official, false).await;
                }
                return;
            }
            let attempts = if mirror_request { 1 } else { CURSEFORGE_API_MAX_ATTEMPTS };
            let fetched = client.fetch(url.clone(), attempts, mirror_request).await
                .and_then(|body| {
                    serde_json::from_slice::<T>(&body)
                        .map(|_| body)
                        .map_err(|error| FetchError::fallbackable(format!(
                            "CurseForge API 响应格式无效：{error}"
                        )))
                });

            match fetched {
                Ok(body) => {
                    if mirror_request {
                        mirror_mark_healthy();
                    }
                    if let Err(error) = cache::store(&cache_key, Arc::<[u8]>::from(body)).await {
                        tracing::debug!("store refreshed CurseForge cache failed: {error}");
                    }
                }
                Err(error) if mirror_request && error.fallback_allowed => {
                    if error.mirror_health_failure {
                        mirror_mark_unhealthy();
                    }
                    tracing::warn!(
                        mirror = %url,
                        reason = %error.message,
                        "CurseForge 镜像后台刷新失败，转用官方 API"
                    );
                    if let Some(official) = client.official_fallback_url(&url)
                        && let Err(error) = client.get_one::<T>(official, false).await
                    {
                        tracing::debug!("CurseForge 官方 API 后台刷新也失败：{}", error.message);
                    }
                }
                Err(error) => tracing::debug!(
                    url = %cache_key,
                    "CurseForge 后台刷新失败：{}",
                    error.message
                ),
            }
        };
        if let Err(error) = crate::tasks::runtime::spawn_io(refresh) {
            tracing::debug!("schedule CurseForge cache refresh failed: {error}");
        }
    }

    async fn fetch(
        &self,
        url: Url,
        max_attempts: usize,
        mirror_request: bool,
    ) -> Result<Vec<u8>, FetchError> {
        let mut last_error = None;

        for attempt in 0..max_attempts {
            let mut request = self.client.get(url.clone());
            if let Some(key) = self.credential_for(&url) {
                request = request.header("x-api-key", key);
            }
            if mirror_request {
                // The official fallback should not wait for three full 20s
                // timeouts on an unreachable mirror.
                request = request.timeout(MIRROR_REQUEST_TIMEOUT);
            }
            match request.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return response
                            .bytes()
                            .await
                            .map(|bytes| bytes.to_vec())
                            .map_err(|error| FetchError::retryable(error.to_string()));
                    }

                    let status = response.status();
                    if attempt + 1 < max_attempts && should_retry_status(status) {
                        last_error = Some(format!("API Error: Status {status}"));
                        tokio::time::sleep(Duration::from_millis(CURSEFORGE_API_RETRY_DELAY_MS))
                            .await;
                        continue;
                    }

                    return Err(FetchError {
                        message: format!("API Error: Status {status}"),
                        retryable: should_retry_status(status),
                        fallback_allowed: should_fallback_status(status),
                        mirror_health_failure: should_open_mirror_circuit(status),
                    });
                }
                Err(error) => {
                    let error_message = error.to_string();
                    if attempt + 1 < max_attempts {
                        last_error = Some(error_message);
                        tokio::time::sleep(Duration::from_millis(CURSEFORGE_API_RETRY_DELAY_MS))
                            .await;
                        continue;
                    }

                    return Err(FetchError::retryable(error_message));
                }
            }
        }

        Err(FetchError::retryable(
            last_error.unwrap_or_else(|| "API request failed".to_string()),
        ))
    }

    pub async fn get_categories(&self) -> Result<Vec<Category>, String> {
        let mut url =
            Url::parse(&format!("{}/categories", self.base_v1)).map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("gameId", GAME_ID);
        let response = self.get::<GetCategoriesResponse>(url).await?;
        Ok(response.data)
    }

    pub async fn get_minecraft_versions(&self) -> Result<Vec<String>, String> {
        let url_primary = Url::parse(&format!("{}/games/{}/versions", self.base_v2, GAME_ID))
            .map_err(|e| e.to_string())?;
        let response = match self.get::<GetVersionsV2Response>(url_primary).await {
            Ok(response) => response,
            Err(err) => {
                // Some mirrors omit v2 versions entirely. We already tried the
                // official fallback when mirror mode is selected; allow browsing
                // without a version filter if the mirror route was missing.
                // Custom sources remain pinned to their configured host.
                if err.starts_with("API Error: Status 404")
                    || err.contains("镜像失败（API Error: Status 404")
                {
                    return Ok(Vec::new());
                }
                return Err(err);
            }
        };

        let mut versions: Vec<String> = response
            .data
            .into_iter()
            .flat_map(|v| v.versions)
            .filter_map(|val| {
                if let Some(s) = val.as_str() {
                    return Some(s.to_string());
                }
                if let Some(obj) = val.as_object() {
                    if let Some(s) = obj.get("name").and_then(|v| v.as_str()) {
                        return Some(s.to_string());
                    }
                    if let Some(s) = obj.get("versionString").and_then(|v| v.as_str()) {
                        return Some(s.to_string());
                    }
                }
                None
            })
            .collect();

        versions.sort();
        versions.dedup();
        versions.sort_by(|a, b| version_compare(b, a));
        Ok(versions)
    }

    pub async fn search_mods(&self, query: SearchModsQuery) -> Result<SearchResponse, String> {
        let mut params: Vec<(String, String)> = Vec::new();
        params.push(("gameId".to_string(), GAME_ID.to_string()));
        if let Some(class_id) = query.class_id {
            params.push(("classId".to_string(), class_id.to_string()));
        }
        if let Some(category_id) = query.category_id {
            params.push(("categoryId".to_string(), category_id.to_string()));
        }
        if let Some(game_version) = query.game_version {
            if !game_version.is_empty() && game_version != "all" {
                params.push(("gameVersion".to_string(), game_version));
            }
        }
        if let Some(search_filter) = query.search_filter {
            if !search_filter.trim().is_empty() {
                params.push(("searchFilter".to_string(), search_filter));
            }
        }
        if let Some(sort_field) = query.sort_field {
            params.push(("sortField".to_string(), sort_field.to_string()));
        }
        if let Some(sort_order) = query.sort_order {
            params.push(("sortOrder".to_string(), sort_order));
        }
        params.push((
            "pageSize".to_string(),
            query.page_size.unwrap_or(20).to_string(),
        ));
        params.push(("index".to_string(), query.index.unwrap_or(0).to_string()));

        let url = Url::parse_with_params(
            &format!("{}/mods/search", self.base_v1),
            params.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .map_err(|e| e.to_string())?;
        self.get::<SearchResponse>(url).await
    }

    pub async fn get_mod_files(
        &self,
        mod_id: i32,
        game_version: Option<String>,
    ) -> Result<GetModFilesResponse, String> {
        let mut params: Vec<(String, String)> = Vec::new();
        if let Some(ver) = game_version {
            if !ver.is_empty() && ver != "all" {
                params.push(("gameVersion".to_string(), ver));
            }
        }
        params.push(("pageSize".to_string(), "50".to_string()));

        let url = Url::parse_with_params(
            &format!("{}/mods/{}/files", self.base_v1, mod_id),
            params.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .map_err(|e| e.to_string())?;
        self.get::<GetModFilesResponse>(url).await
    }

    pub async fn get_mod(&self, mod_id: i32) -> Result<CurseForgeMod, String> {
        let url =
            Url::parse(&format!("{}/mods/{}", self.base_v1, mod_id)).map_err(|e| e.to_string())?;
        let response = self.get::<GetModResponse>(url).await?;
        Ok(response.data)
    }

    pub async fn get_mod_description(&self, mod_id: i32) -> Result<String, String> {
        let url = Url::parse(&format!("{}/mods/{}/description", self.base_v1, mod_id))
            .map_err(|e| e.to_string())?;
        let response = self.get::<GetStringResponse>(url).await?;
        Ok(response.data)
    }
}

pub fn prewarm() -> Result<(), String> {
    let client = CurseForgeClient::new()?;
    crate::tasks::runtime::spawn_io(async move {
        let (categories, versions) =
            tokio::join!(client.get_categories(), client.get_minecraft_versions());
        if let Err(error) = categories {
            tracing::debug!("CurseForge category prewarm failed: {error}");
        }
        if let Err(error) = versions {
            tracing::debug!("CurseForge version prewarm failed: {error}");
        }
    })?;
    Ok(())
}

struct FetchError {
    message: String,
    retryable: bool,
    fallback_allowed: bool,
    mirror_health_failure: bool,
}

impl FetchError {
    fn retryable(message: String) -> Self {
        Self {
            message,
            retryable: true,
            fallback_allowed: true,
            mirror_health_failure: true,
        }
    }

    fn fallbackable(message: String) -> Self {
        Self {
            message,
            retryable: false,
            fallback_allowed: true,
            mirror_health_failure: true,
        }
    }

    fn cooldown() -> Self {
        Self {
            message: "CurseForge 镜像正在故障冷却中".to_string(),
            retryable: false,
            fallback_allowed: true,
            mirror_health_failure: false,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct SearchModsQuery {
    pub class_id: Option<i32>,
    pub category_id: Option<i32>,
    pub game_version: Option<String>,
    pub search_filter: Option<String>,
    pub sort_field: Option<i32>,
    pub sort_order: Option<String>,
    pub page_size: Option<u32>,
    pub index: Option<u32>,
}

fn build_http_client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn auth_cache_tag(key: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hash);
    hash.finish()
}

fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn should_fallback_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            reqwest::StatusCode::REQUEST_TIMEOUT
                | reqwest::StatusCode::TOO_MANY_REQUESTS
                | reqwest::StatusCode::NOT_FOUND
                | reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::FORBIDDEN
        )
}

// An absent API route is local to that route, not evidence that all
// unrelated mirror routes are down. Do not globally open the circuit on 404.
fn should_open_mirror_circuit(status: reqwest::StatusCode) -> bool {
    should_fallback_status(status) && status != reqwest::StatusCode::NOT_FOUND
}

fn official_fallback_url(mirror_url: &Url) -> Option<Url> {
    if mirror_url.scheme() != "https" || mirror_url.host_str() != Some("mod.mcimirror.top") {
        return None;
    }
    // Do not accidentally replace a similarly-named path on the mirror host.
    let suffix = mirror_url.path().strip_prefix("/curseforge")?;
    if !suffix.starts_with("/v1/") && !suffix.starts_with("/v2/") {
        return None;
    }
    let mut official = Url::parse(CURSEFORGE_OFFICIAL_BASE).ok()?;
    official.set_path(suffix);
    official.set_query(mirror_url.query());
    Some(official)
}

fn mirror_in_cooldown() -> bool {
    MIRROR_UNHEALTHY_UNTIL
        .lock()
        .map(|until| until.is_some_and(|deadline| deadline > Instant::now()))
        .unwrap_or(false)
}

fn mirror_mark_unhealthy() {
    if let Ok(mut until) = MIRROR_UNHEALTHY_UNTIL.lock() {
        *until = Some(Instant::now() + MIRROR_FAILOVER_COOLDOWN);
    }
}

fn mirror_mark_healthy() {
    if let Ok(mut until) = MIRROR_UNHEALTHY_UNTIL.lock() {
        *until = None;
    }
}

// Resolve at client construction rather than pinning the first launch setting for
// the process lifetime. HTTP connection pooling remains shared by build_http_client().
fn build_base_urls() -> Result<(String, String, bool), String> {
    let (source, custom_base) = read_config()
        .ok()
        .map(|config| (
            config.launcher.download.curseforge_api_source,
            config.launcher.download.curseforge_api_base,
        ))
        .unwrap_or_default();
    let (v1, v2) = resolve_base_urls(&source, &custom_base)?;
    Ok((v1, v2, mirror_fallback_enabled(&source)))
}

// Explicit custom URLs never inherit the mirror's automatic fallback policy,
// even when the supplied URL happens to equal the bundled mirror URL.
fn mirror_fallback_enabled(source: &str) -> bool {
    !matches!(
        source.trim().to_ascii_lowercase().as_str(),
        "custom" | "official"
    )
}

fn resolve_base_urls(source: &str, custom_base: &str) -> Result<(String, String), String> {
    let base = match source.trim().to_ascii_lowercase().as_str() {
        "official" => CURSEFORGE_OFFICIAL_BASE,
        "custom" => {
            let base = custom_base.trim().trim_end_matches('/');
            let parsed = Url::parse(base)
                .map_err(|_| "CurseForge 自定义 API 源必须是有效的 HTTP(S) 地址".to_string())?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err("CurseForge 自定义 API 源必须是不含凭据、查询或片段的 HTTP(S) 基础地址".to_string());
            }
            base
        }
        _ => CURSEFORGE_MIRROR_BASE,
    };
    Ok((format!("{base}/v1"), format!("{base}/v2")))
}

#[cfg(test)]
mod api_source_tests {
    use super::{
        CURSEFORGE_MIRROR_BASE, CURSEFORGE_OFFICIAL_BASE,
        official_fallback_url, mirror_fallback_enabled, resolve_base_urls,
        should_fallback_status, should_open_mirror_circuit,
    };
    use reqwest::StatusCode;
    use url::Url;

    #[test]
    fn custom_source_keeps_its_prefix_and_trims_trailing_slash() {
        assert_eq!(
            resolve_base_urls("custom", " https://example.com/cf/ ").unwrap(),
            ("https://example.com/cf/v1".to_string(), "https://example.com/cf/v2".to_string()),
        );
        assert_eq!(
            resolve_base_urls("official", "https://ignored.example").unwrap().0,
            "https://api.curseforge.com/v1"
        );
        assert_eq!(
            resolve_base_urls("mirror", "").unwrap().0,
            format!("{CURSEFORGE_MIRROR_BASE}/v1")
        );
    }


    #[test]
    fn mirror_fallback_preserves_version_path_and_encoded_queries() {
        let mirror = Url::parse(
            "https://mod.mcimirror.top/curseforge/v1/mods/search?gameId=78022&searchFilter=abc%20def&pageSize=20"
        ).unwrap();
        let official = official_fallback_url(&mirror).unwrap();
        assert_eq!(
            official.as_str(),
            format!(
                "{CURSEFORGE_OFFICIAL_BASE}/v1/mods/search?gameId=78022&searchFilter=abc%20def&pageSize=20"
            )
        );
        let versions = Url::parse(
            "https://mod.mcimirror.top/curseforge/v2/games/78022/versions"
        ).unwrap();
        assert_eq!(
            official_fallback_url(&versions).unwrap().as_str(),
            format!("{CURSEFORGE_OFFICIAL_BASE}/v2/games/78022/versions")
        );
    }

    #[test]
    fn fallback_rejects_unrelated_hosts_and_lookalike_paths() {
        for url in [
            "https://api.curseforge.com/v1/mods/search",
            "https://other.example/curseforge/v1/mods/search",
            "https://mod.mcimirror.top/curseforge-other/v1/mods/search",
            "https://mod.mcimirror.top/curseforge/v3/mods/search",
        ] {
            assert!(official_fallback_url(&Url::parse(url).unwrap()).is_none(), "{url}");
        }
    }

    #[test]
    fn explicit_custom_and_official_sources_never_use_mirror_fallback() {
        assert!(mirror_fallback_enabled("mirror"));
        assert!(mirror_fallback_enabled(""));
        assert!(!mirror_fallback_enabled("official"));
        assert!(!mirror_fallback_enabled("custom"));
        assert!(!mirror_fallback_enabled(" CUSTOM "));
    }

    #[test]
    fn fallback_distinguishes_mirror_failures_from_bad_queries() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(should_fallback_status(status), "{status}");
            assert_eq!(
                should_open_mirror_circuit(status),
                status != StatusCode::NOT_FOUND,
                "{status}"
            );
        }
        for status in [StatusCode::BAD_REQUEST, StatusCode::UNPROCESSABLE_ENTITY] {
            assert!(!should_fallback_status(status), "{status}");
            assert!(!should_open_mirror_circuit(status), "{status}");
        }
    }

    #[test]
    fn api_auth_cache_tags_distinguish_keys() {
        assert_ne!(super::auth_cache_tag(Some("one")), super::auth_cache_tag(Some("two")));
        assert_ne!(super::auth_cache_tag(None), super::auth_cache_tag(Some("one")));
    }

    #[test]
    fn custom_source_rejects_invalid_or_ambiguous_base_urls() {
        for bad in ["", "file:///etc/passwd", "https://a.test/?key=x", "https://a.test/#x", "https://user:pass@a.test", "not a url"] {
            assert!(resolve_base_urls("custom", bad).is_err(), "{bad}");
        }
    }
}

fn version_compare(v1: &str, v2: &str) -> std::cmp::Ordering {
    let parts1: Vec<&str> = v1.split('.').collect();
    let parts2: Vec<&str> = v2.split('.').collect();
    let len = std::cmp::max(parts1.len(), parts2.len());

    for i in 0..len {
        let p1 = parts1
            .get(i)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let p2 = parts2
            .get(i)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if p1 != p2 {
            return p1.cmp(&p2);
        }
    }
    std::cmp::Ordering::Equal
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeMod {
    pub id: i32,
    pub game_id: i32,
    pub name: String,
    pub slug: String,
    pub summary: Option<String>,
    pub download_count: f64,
    pub logo: Option<Logo>,

    #[serde(default)]
    pub authors: Vec<Author>,

    #[serde(default)]
    pub categories: Vec<Category>,

    pub class_id: Option<i32>,
    pub date_modified: String,
    pub date_created: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFile {
    pub id: i32,
    pub display_name: String,
    pub file_name: String,
    pub file_length: u64,
    pub download_url: Option<String>,

    #[serde(default)]
    pub game_versions: Vec<String>,

    pub file_date: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Logo {
    pub url: String,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Author {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: i32,
    pub name: String,
    pub slug: String,
    pub icon_url: Option<String>,
    pub is_class: Option<bool>,
    pub class_id: Option<i32>,
    pub parent_category_id: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub result_count: u32,
    pub total_count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResponse {
    pub data: Vec<CurseForgeMod>,
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetModFilesResponse {
    pub data: Vec<CurseForgeFile>,
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetModResponse {
    pub data: CurseForgeMod,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetStringResponse {
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetCategoriesResponse {
    pub data: Vec<Category>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetVersionsV2Response {
    pub data: Vec<GameVersionTypeV2>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameVersionTypeV2 {
    #[serde(rename = "type")]
    pub type_id: i32,
    #[serde(default)]
    pub versions: Vec<serde_json::Value>,
}
