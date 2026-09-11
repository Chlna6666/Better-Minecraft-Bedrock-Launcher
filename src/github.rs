use crate::config::config::{GithubConfig, GithubSource};

// Public mirror templates used by BedrockBoot 2.0-develop's SourceList.cs.
const BUILT_IN_MIRRORS: &[&str] = &[
    "https://github1.roundstudio.top/{url}",
    "https://gh.tianpao.top/{url}",
    "https://gh.tiouo.cc/{route}",
    "https://gh-proxy.com/{url}",
    "https://gh-proxy.net/{url}",
    "https://gh-proxy.org/{url}",
    "https://gitproxy.click/{url}",
];

/// Builds the configured candidates for a public GitHub file URL.
///
/// Non-GitHub URLs and malformed URLs are returned unchanged. The original
/// GitHub URL is always retained as the final fallback.
pub(crate) fn download_urls(url: &str, config: &GithubConfig) -> Vec<String> {
    if !is_github_file_url(url) {
        return vec![url.to_string()];
    }

    let mut urls = Vec::with_capacity(BUILT_IN_MIRRORS.len() + 1);
    match config.source {
        GithubSource::Auto => {
            for pattern in BUILT_IN_MIRRORS {
                push_unique(&mut urls, render_mirror(pattern, url));
            }
        }
        GithubSource::Direct => {}
        GithubSource::Custom => {
            let pattern = config.custom_mirror.trim();
            if !pattern.is_empty() {
                push_unique(&mut urls, render_mirror(pattern, url));
            }
        }
    }
    push_unique(&mut urls, url.to_string());
    urls
}

/// Reads the current application setting and builds GitHub download candidates.
///
/// # Errors
///
/// Returns an error when the startup configuration cache is unavailable.
pub(crate) fn configured_download_urls(url: &str) -> Result<Vec<String>, String> {
    let config = crate::config::config::read_config()
        .map_err(|error| format!("读取 GitHub 下载配置失败：{error}"))?;
    Ok(download_urls(url, &config.launcher.download.github))
}

/// Sends a GET request through the configured GitHub file mirrors.
///
/// # Errors
///
/// Returns an error when configuration cannot be read or every candidate
/// fails to produce a successful HTTP response.
pub(crate) async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    let urls = configured_download_urls(url)?;
    let mut errors = Vec::with_capacity(urls.len());
    for candidate in urls {
        match client.get(&candidate).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => return Ok(response),
                Err(error) => errors.push(format!("{candidate}: {error}")),
            },
            Err(error) => errors.push(format!("{candidate}: {error}")),
        }
    }
    Err(errors.join("; "))
}

fn is_github_file_url(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    matches!(
        url.host_str(),
        Some(
            "github.com"
                | "raw.githubusercontent.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
        )
    )
}

fn render_mirror(pattern: &str, url: &str) -> String {
    let route = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .unwrap_or(url);
    if pattern.contains("{url}") || pattern.contains("{route}") {
        return pattern.replace("{url}", url).replace("{route}", route);
    }
    format!("{}/{url}", pattern.trim_end_matches('/'))
}

fn push_unique(urls: &mut Vec<String>, url: String) {
    if !urls.iter().any(|candidate| candidate == &url) {
        urls.push(url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE_URL: &str =
        "https://github.com/LiteLDev/LeviLamina/releases/download/v1.0/file.zip";

    #[test]
    fn auto_uses_bedrock_boot_mirrors_before_github() {
        let urls = download_urls(RELEASE_URL, &GithubConfig::default());

        assert_eq!(urls.len(), BUILT_IN_MIRRORS.len() + 1);
        assert_eq!(
            urls.first().map(String::as_str),
            Some(
                "https://github1.roundstudio.top/https://github.com/LiteLDev/LeviLamina/releases/download/v1.0/file.zip"
            )
        );
        assert_eq!(urls.last().map(String::as_str), Some(RELEASE_URL));
        assert_eq!(
            urls.get(2).map(String::as_str),
            Some("https://gh.tiouo.cc/LiteLDev/LeviLamina/releases/download/v1.0/file.zip")
        );
    }

    #[test]
    fn custom_template_is_followed_by_github_fallback() {
        let config = GithubConfig {
            source: GithubSource::Custom,
            custom_mirror: "https://mirror.example/{url}".to_string(),
        };

        assert_eq!(
            download_urls(RELEASE_URL, &config),
            vec![
                format!("https://mirror.example/{RELEASE_URL}"),
                RELEASE_URL.to_string()
            ]
        );
    }

    #[test]
    fn custom_base_url_is_used_as_a_prefix() {
        let config = GithubConfig {
            source: GithubSource::Custom,
            custom_mirror: "https://mirror.example/".to_string(),
        };

        assert_eq!(
            download_urls(RELEASE_URL, &config)[0],
            format!("https://mirror.example/{RELEASE_URL}")
        );
    }

    #[test]
    fn non_github_url_is_not_rewritten() {
        let url = "https://example.com/file.zip";

        assert_eq!(download_urls(url, &GithubConfig::default()), vec![url]);
    }
}
