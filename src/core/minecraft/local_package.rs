use std::path::{Path, PathBuf};

use crate::archive::api::import_appx;
use crate::core::minecraft::gdk::unpack::start_unpack_gdk_task;

pub const LOCAL_GAME_PACKAGE_EXTENSIONS: &[&str] = &["appx", "zip", "msixvc"];

pub async fn start_local_game_package_import(path: impl Into<PathBuf>) -> Result<String, String> {
    let path = path.into();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("无法识别安装包格式: {}", path.display()))?;
    let path_text = path
        .to_str()
        .ok_or_else(|| format!("安装包路径不是有效文本: {}", path.display()))?;

    if extension.eq_ignore_ascii_case("msixvc") {
        let folder_name = package_folder_name(&path);
        return start_unpack_gdk_task(path_text, folder_name);
    }
    if extension.eq_ignore_ascii_case("appx") || extension.eq_ignore_ascii_case("zip") {
        return import_appx(path_text.to_string(), None).await;
    }

    Err(format!(
        "不支持的游戏版本安装包格式: .{extension}（支持 APPX、MSIXVC 和 ZIP）"
    ))
}

fn package_folder_name(path: &Path) -> &str {
    path.file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("ImportedGDK")
}
