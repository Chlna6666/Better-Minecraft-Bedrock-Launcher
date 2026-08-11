use super::AuthError;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLIENT_ID: &str = "00000000402b5328";
const SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";
const DEVICE_CODE_ENDPOINT: &str = "https://login.live.com/oauth20_connect.srf";
const TOKEN_ENDPOINT: &str = "https://login.live.com/oauth20_token.srf";
const REMOTE_CONNECT_ENDPOINT: &str = "https://login.live.com/oauth20_remoteconnect.srf";
const DEFAULT_ACCESS_TOKEN_LIFETIME_SECONDS: u64 = 3600;
const MIN_ACCESS_TOKEN_REMAINING_SECONDS: u64 = 30;

struct CachedAccessToken {
    token: SecretString,
    expires_at_epoch: u64,
}

static CURRENT_ACCESS_TOKEN: OnceLock<Mutex<Option<CachedAccessToken>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub(super) struct DeviceCode {
    pub(super) device_code: SecretString,
    pub(super) user_code: String,
    pub(super) verification_url: String,
    pub(super) interval: Duration,
    pub(super) expires_in: Duration,
}

#[derive(Debug)]
pub(super) struct MsaToken {
    pub(super) access_token: SecretString,
    pub(super) refresh_token: SecretString,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    interval: Option<u64>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

pub(super) fn client() -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .user_agent("BMCBL")
        .timeout(Duration::from_secs(30))
        .https_only(true)
        .build()
        .map_err(AuthError::Http)
}

/// Executes `operation` while borrowing the latest short-lived Microsoft
/// access token. The refresh token never enters this cache and is never exposed
/// to BLoader. The access token is only made available while it still has a
/// small validity margin.
pub(super) fn with_current_access_token<T>(
    operation: impl FnOnce(&str, u64) -> T,
) -> Option<T> {
    let cache = CURRENT_ACCESS_TOKEN.get_or_init(|| Mutex::new(None));
    let guard = cache.lock().ok()?;
    let cached = guard.as_ref()?;
    let now = now_epoch();
    if cached.expires_at_epoch <= now.saturating_add(MIN_ACCESS_TOKEN_REMAINING_SECONDS) {
        return None;
    }
    Some(operation(
        cached.token.expose_secret(),
        cached.expires_at_epoch,
    ))
}

fn cache_access_token(token: &SecretString, expires_at_epoch: u64) {
    let cache = CURRENT_ACCESS_TOKEN.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(CachedAccessToken {
            token: SecretString::from(token.expose_secret().to_string()),
            expires_at_epoch,
        });
    }
}

pub(super) async fn request_device_code(client: &reqwest::Client) -> Result<DeviceCode, AuthError> {
    let response = client
        .post(DEVICE_CODE_ENDPOINT)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("response_type", "device_code"),
        ])
        .send()
        .await
        .map_err(AuthError::Http)?;
    let status = response.status();
    let payload: DeviceCodeResponse = response.json().await.map_err(AuthError::Http)?;
    let device_code = payload.device_code.filter(|value| !value.is_empty());
    let user_code = payload.user_code.filter(|value| !value.is_empty());
    match (device_code, user_code) {
        (Some(device_code), Some(user_code)) if status.is_success() => Ok(DeviceCode {
            device_code: SecretString::from(device_code),
            verification_url: remote_connect_url(&user_code),
            user_code,
            interval: Duration::from_secs(payload.interval.unwrap_or(5).max(1)),
            expires_in: Duration::from_secs(payload.expires_in.unwrap_or(900).max(60)),
        }),
        _ => Err(AuthError::OAuth(
            payload
                .error_description
                .or(payload.error)
                .unwrap_or_else(|| format!("设备代码请求失败（HTTP {status}）")),
        )),
    }
}

pub(super) async fn poll_device_code<F>(
    client: &reqwest::Client,
    code: &DeviceCode,
    mut is_cancelled: F,
) -> Result<MsaToken, AuthError>
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + code.expires_in;
    let mut interval = code.interval;
    while Instant::now() < deadline {
        tokio::time::sleep(interval).await;
        if is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        let response = client
            .post(TOKEN_ENDPOINT)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "device_code"),
                ("device_code", code.device_code.expose_secret()),
            ])
            .send()
            .await
            .map_err(AuthError::Http)?;
        let payload: TokenResponse = response.json().await.map_err(AuthError::Http)?;
        match payload.error.as_deref() {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                interval = interval.saturating_add(Duration::from_secs(5));
                continue;
            }
            Some(_) => {
                return Err(AuthError::OAuth(
                    payload
                        .error_description
                        .or(payload.error)
                        .unwrap_or_else(|| "Microsoft 登录失败".to_string()),
                ));
            }
            None => {}
        }
        return token_from_response(payload);
    }
    Err(AuthError::TimedOut)
}

pub(super) async fn refresh(
    client: &reqwest::Client,
    refresh_token: &SecretString,
) -> Result<MsaToken, AuthError> {
    let response = client
        .post(TOKEN_ENDPOINT)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose_secret()),
        ])
        .send()
        .await
        .map_err(AuthError::Http)?;
    let payload: TokenResponse = response.json().await.map_err(AuthError::Http)?;
    if let Some(error) = payload.error {
        return Err(AuthError::OAuth(payload.error_description.unwrap_or(error)));
    }
    token_from_response(payload)
}

fn token_from_response(payload: TokenResponse) -> Result<MsaToken, AuthError> {
    let access_token = payload
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| AuthError::InvalidResponse("缺少 Microsoft access token"))?;
    let refresh_token = payload
        .refresh_token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| AuthError::InvalidResponse("缺少 Microsoft refresh token"))?;
    let access_token = SecretString::from(access_token);
    let expires_at_epoch = now_epoch().saturating_add(
        payload
            .expires_in
            .unwrap_or(DEFAULT_ACCESS_TOKEN_LIFETIME_SECONDS)
            .max(60),
    );
    cache_access_token(&access_token, expires_at_epoch);
    Ok(MsaToken {
        access_token,
        refresh_token: SecretString::from(refresh_token),
    })
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn remote_connect_url(user_code: &str) -> String {
    format!("{REMOTE_CONNECT_ENDPOINT}?otc={user_code}")
}

#[cfg(test)]
mod tests {
    use super::remote_connect_url;

    #[test]
    fn remote_connect_url_contains_user_code() {
        assert_eq!(
            remote_connect_url("ABCD-EFGH"),
            "https://login.live.com/oauth20_remoteconnect.srf?otc=ABCD-EFGH"
        );
    }
}
