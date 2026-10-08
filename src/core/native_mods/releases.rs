use serde::Deserialize;

use super::github_repository;

/// A published GitHub release and its downloadable assets; no game-version conversion is performed.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Release {
    /// The exact repository tag used by the catalog's `{{tag}}` URL template.
    pub(crate) tag_name: String,
    #[serde(default)]
    draft: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    browser_download_url: String,
}

impl Release {
    /// Whether this release contains the asset addressed by a catalog URL template.
    pub(crate) fn supports(&self, template: &str) -> bool {
        let url = template.replace("{{tag}}", &self.tag_name);
        self.assets
            .iter()
            .any(|asset| asset.browser_download_url == url)
    }
}

/// Reads all published releases, including prereleases, without changing the catalog or game files.
///
/// # Errors
/// Returns an error for a non-GitHub repository, HTTP failure, or malformed release response.
pub(crate) async fn releases(repository: &str) -> Result<Vec<Release>, String> {
    let repository = github_repository(repository)
        .ok_or_else(|| format!("原生 Mod 仓库不是 GitHub 地址：{repository}"))?;
    let client = crate::http::proxy::get_client_for_proxy().map_err(|error| error.to_string())?;
    let url = format!("https://api.github.com/repos/{repository}/releases");
    let mut releases = Vec::new();
    for page in 1u32.. {
        let response = client
            .get(format!("{url}?per_page=100&page={page}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|error| format!("获取 GitHub Releases 失败：{error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "GitHub Releases 返回错误状态：{}",
                response.status()
            ));
        }
        let batch = response
            .json::<Vec<Release>>()
            .await
            .map_err(|error| format!("解析 GitHub Releases 失败：{error}"))?;
        let last_page = batch.len() < 100;
        releases.extend(
            batch
                .into_iter()
                .filter(|release| !release.draft && !release.tag_name.trim().is_empty()),
        );
        if last_page {
            break;
        }
    }
    Ok(releases)
}

#[cfg(test)]
mod tests {
    use super::Release;

    #[test]
    fn only_offers_releases_with_the_requested_asset() {
        let release: Release = serde_json::from_str(
            r#"{
            "tag_name":"v1.0", "assets":[{"browser_download_url":
            "https://github.com/example/mod/releases/download/v1.0/Mod.dll"}]
        }"#,
        )
        .expect("valid release fixture");
        assert!(
            release.supports("https://github.com/example/mod/releases/download/{{tag}}/Mod.dll")
        );
        assert!(
            !release.supports("https://github.com/example/mod/releases/download/{{tag}}/Other.dll")
        );
    }
}
