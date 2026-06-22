use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

// 引入 build.rs 自动生成的图片与图标资源
include!(concat!(env!("OUT_DIR"), "/image_assets.rs"));
include!(concat!(env!("OUT_DIR"), "/icon_assets.rs"));

/// Application asset bundle for GPUI.
///
/// - Provides embedded app icons under `icons/**`.
/// - Provides a small set of app-owned embedded assets (images, etc).
/// - Auto-discovers all images under `assets/images/` at build time.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        if let Some(bytes) = crate::assets::generated::get(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }

        if path.starts_with("icons/") {
            if let Some(bytes) = load_icon_asset(path)? {
                return Ok(Some(bytes));
            }

            // 兜底：开发环境仍允许从本地文件读取，避免影响增量调试。
            if path == "icons/logo.svg" || path == "icons/logo.png" || path == "icons/32x32.png" {
                let manifest_dir = std::env!("CARGO_MANIFEST_DIR");
                let logo_file = if path.ends_with(".svg") {
                    "logo.svg"
                } else if path == "icons/32x32.png" {
                    "32x32.png"
                } else {
                    "logo.png"
                };
                let logo_path = std::path::Path::new(manifest_dir)
                    .join("assets")
                    .join("icons")
                    .join(logo_file);
                if logo_path.exists() {
                    let bytes = std::fs::read(&logo_path)
                        .map_err(|e| anyhow::anyhow!("Failed to read logo: {}", e))?;
                    return Ok(Some(Cow::Owned(bytes)));
                }
            }

            return Ok(None);
        }

        if path.starts_with("lucide/") {
            return lucide_gpui::Assets.load(path);
        }

        // 自动处理所有 images/ 路径
        if path.starts_with("images/") {
            return load_image_asset(path);
        }

        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.starts_with("icons/") || path == "icons" {
            return Ok(list_icon_assets());
        }
        if path.starts_with("lucide/") || path == "lucide" {
            return lucide_gpui::Assets.list(path);
        }

        // 自动处理所有 images/ 路径
        if path.is_empty() || path == "images" || path == "images/" {
            return Ok(list_image_assets());
        }

        // 支持子目录过滤
        if path.starts_with("images/") {
            let all = list_image_assets();
            let prefix = path.trim_end_matches('/');
            let filtered = all.into_iter().filter(|p| p.starts_with(prefix)).collect();
            return Ok(filtered);
        }

        Ok(Vec::new())
    }
}
