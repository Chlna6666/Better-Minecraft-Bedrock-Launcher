use super::*;

pub(super) fn normalize_local_background_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(stripped) = trimmed.strip_prefix("file:///") {
        return Some(stripped.to_string());
    }
    if let Some(stripped) = trimmed.strip_prefix("file://") {
        return Some(stripped.trim_start_matches('/').to_string());
    }

    Some(trimmed.to_string())
}

pub(super) fn has_supported_background_extension(source: &str) -> bool {
    let cleaned = source
        .split('?')
        .next()
        .unwrap_or(source)
        .split('#')
        .next()
        .unwrap_or(source)
        .to_ascii_lowercase();

    const SUPPORTED: &[&str] = &[".webp", ".gif", ".png", ".apng", ".jpg", ".jpeg", ".bmp"];
    SUPPORTED.iter().any(|ext| cleaned.ends_with(ext))
}

pub(super) fn network_background_cache_key(url: &str) -> String {
    let trimmed = url.trim();
    let (without_fragment, fragment) = match trimmed.split_once('#') {
        Some((head, tail)) => (head, Some(tail)),
        None => (trimmed, None),
    };
    let (base, query) = match without_fragment.split_once('?') {
        Some((head, tail)) => (head, Some(tail)),
        None => (without_fragment, None),
    };

    let mut rebuilt = String::from(base);
    if let Some(query) = query {
        let filtered = query
            .split('&')
            .filter(|part| !part.is_empty())
            .filter(|part| part.split('=').next().unwrap_or_default() != "_bm_refresh")
            .collect::<Vec<_>>();
        if !filtered.is_empty() {
            rebuilt.push('?');
            rebuilt.push_str(&filtered.join("&"));
        }
    }

    if let Some(fragment) = fragment {
        rebuilt.push('#');
        rebuilt.push_str(fragment);
    }

    rebuilt
}

#[derive(Clone)]
pub(super) enum BackgroundSource {
    None,
    FetchedImage(Arc<Image>),
    Embedded(SharedString),
    LocalPath(PathBuf),
    NetworkUrl(SharedString),
}

pub(super) fn background_resource(source: &BackgroundSource) -> Option<Resource> {
    match source {
        BackgroundSource::None => None,
        BackgroundSource::Embedded(path) => Some(Resource::Embedded(path.clone())),
        BackgroundSource::LocalPath(path) => Some(Resource::Path(path.clone().into())),
        BackgroundSource::NetworkUrl(url) => Some(Resource::Uri(url.clone().into())),
        BackgroundSource::FetchedImage(_) => None,
    }
}

pub(super) fn default_background_source() -> BackgroundSource {
    BackgroundSource::Embedded(SharedString::from("images/background.webp"))
}

pub(super) fn uses_embedded_default_background(
    background_option: &str,
    local_image_path: &str,
    network_image_url: &str,
    network_image_refresh_nonce: u64,
) -> bool {
    matches!(
        resolve_background_source_from_values(
            background_option,
            local_image_path,
            network_image_url,
            network_image_refresh_nonce,
        ),
        BackgroundSource::Embedded(ref path) if path.as_ref() == "images/background.webp"
    )
}

pub(super) struct BackgroundSettingsSnapshot {
    pub(super) loaded: bool,
    pub(super) background_option: String,
    pub(super) local_image_path: String,
    pub(super) network_image_url: String,
    pub(super) background_blur: f32,
    pub(super) network_image_refresh_nonce: u64,
}

pub(super) struct PreparedBackground {
    pub(super) display_background: Option<BackgroundSource>,
}

pub(super) fn resolve_background_source_from_values(
    background_option: &str,
    local_image_path: &str,
    network_image_url: &str,
    network_image_refresh_nonce: u64,
) -> BackgroundSource {
    match background_option.trim().to_ascii_lowercase().as_str() {
        "local" => normalize_local_background_path(local_image_path)
            .filter(|source| has_supported_background_extension(source))
            .map(PathBuf::from)
            .map(BackgroundSource::LocalPath)
            .unwrap_or_else(default_background_source),
        "network" => {
            let url = network_image_url.trim();
            if url.starts_with("http://") || url.starts_with("https://") {
                let refreshed = if network_image_refresh_nonce > 0 {
                    let separator = if url.contains('?') { '&' } else { '?' };
                    format!("{url}{separator}_bm_refresh={network_image_refresh_nonce}")
                } else {
                    url.to_string()
                };
                BackgroundSource::NetworkUrl(SharedString::from(refreshed))
            } else {
                default_background_source()
            }
        }
        _ => default_background_source(),
    }
}

#[cfg(test)]
mod tests {
    use super::{network_background_cache_key, uses_embedded_default_background};

    #[test]
    fn only_uses_embedded_default_background_when_source_resolves_to_default() {
        assert!(uses_embedded_default_background("default", "", "", 0));
        assert!(uses_embedded_default_background(
            "default",
            "C:\\demo\\custom.webp",
            "https://example.com/custom.webp",
            99
        ));
        assert!(!uses_embedded_default_background(
            "local",
            "C:\\demo\\custom.webp",
            "",
            0
        ));
    }

    #[test]
    fn network_cache_key_drops_background_refresh_nonce() {
        assert_eq!(
            network_background_cache_key("https://example.com/bg.webp?a=1&_bm_refresh=42#hero"),
            "https://example.com/bg.webp?a=1#hero"
        );
    }
}
