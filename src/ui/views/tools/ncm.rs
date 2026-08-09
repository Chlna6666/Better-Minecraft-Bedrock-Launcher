use crate::ui::components::icon::themed_icon;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::path::{Path, PathBuf};

pub(super) fn render_ncm_converter(
    colors: &ThemeColors,
    state: &ToolsPageState,
) -> impl IntoElement {
    crate::ui::components::page_shell::split_content_panel(colors)
        .overflow_y_scrollbar()
        .scrollbar_width(px(0.))
        .p(px(14.))
        .child(
            crate::ui::components::page_shell::glass_card(colors)
                .w_full()
                .max_w(px(760.))
                .p(px(20.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(themed_icon(
                            lucide_icons::icon_download(),
                            19.0,
                            colors.accent,
                        ))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child("NCM 快速解析与导出"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(19.))
                        .text_color(colors.text_secondary)
                        .child("由已安装的 ncm-bridge 解码器处理。源文件不会被修改，导出完成后可直接用普通播放器打开。"),
                )
                .child(
                    div()
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .border_1()
                        .border_color(Hsla { a: 0.18, ..colors.border })
                        .bg(Hsla { a: 0.45, ..colors.surface })
                        .px(px(13.))
                        .py(px(11.))
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(state.ncm_conversion_status.clone()),
                )
                .when_some(state.ncm_conversion_error.clone(), |this, error| {
                    this.child(
                        div()
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .bg(Hsla { a: 0.10, ..colors.danger })
                            .px(px(13.))
                            .py(px(10.))
                            .text_size(px(12.))
                            .text_color(colors.danger)
                            .child(error),
                    )
                })
                .child(render_convert_button(colors, state.ncm_conversion_running)),
        )
}

fn render_convert_button(colors: &ThemeColors, busy: bool) -> Stateful<Div> {
    div()
        .id("tools-ncm-convert")
        .w(px(190.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(if busy {
            colors.surface_hover
        } else {
            colors.accent
        })
        .px(px(15.))
        .py(px(11.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if busy {
            colors.text_muted
        } else {
            colors.btn_primary_text
        })
        .when(!busy, |this| {
            this.cursor_pointer()
                .hover(|style| style.opacity(0.90))
                .active(|style| style.scale(0.98))
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    start_conversion(cx);
                })
        })
        .child(if busy {
            "正在解析…"
        } else {
            "选择 NCM 并导出"
        })
}

fn start_conversion(cx: &mut App) {
    let started = cx.update_global(|state: &mut ToolsPageState, _cx| {
        if state.ncm_conversion_running {
            return false;
        }
        state.ncm_conversion_running = true;
        state.ncm_conversion_error = None;
        state.ncm_conversion_status = SharedString::from("请选择要解析的 NCM 文件。");
        true
    });
    if !started {
        return;
    }

    cx.spawn(async move |cx| {
        let result = convert_selected_file(cx).await;
        cx.update_global(|state: &mut ToolsPageState, _cx| {
            state.ncm_conversion_running = false;
            match result {
                Ok(Some(output)) => {
                    state.ncm_conversion_status =
                        SharedString::from(format!("已导出：{}", output.display()));
                    state.ncm_conversion_error = None;
                }
                Ok(None) => {
                    state.ncm_conversion_status = SharedString::from("已取消导出。");
                }
                Err(error) => {
                    state.ncm_conversion_status = SharedString::from("解析或导出失败。");
                    state.ncm_conversion_error = Some(SharedString::from(format!("{error:#}")));
                }
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

async fn convert_selected_file(cx: &mut AsyncApp) -> anyhow::Result<Option<PathBuf>> {
    let source = crate::tasks::runtime::run_io_blocking(|| {
        crate::utils::file_picker::pick_file_path_with_filter("NCM Music", &["ncm"])
    })
    .await
    .map_err(anyhow::Error::msg)?
    .map(PathBuf::from);
    let Some(source) = source else {
        return Ok(None);
    };

    let decoder = cx
        .update(|cx| {
            cx.update_global(
                |registry: &mut crate::plugins::runtime::PluginRegistry, _cx| {
                    registry
                        .audio_decoders()
                        .into_iter()
                        .find(|decoder| decoder.supports_extension("ncm"))
                },
            )
        })?
        .ok_or_else(|| anyhow::anyhow!("未找到 NCM 解码器，请先安装并启用 ncm-bridge"))?;

    let temporary_path = temporary_output_path(&source);
    let decoded = crate::tasks::runtime::run_io_blocking({
        let source = source.clone();
        let temporary_path = temporary_path.clone();
        move || {
            if let Some(parent) = temporary_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            decoder.decode_to_path(&source, &temporary_path)
        }
    })
    .await
    .map_err(anyhow::Error::msg)?;
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(error) => {
            remove_temporary_file(&temporary_path);
            return Err(error);
        }
    };

    let extension = decoded.format_extension.to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac"
    ) {
        remove_temporary_file(&temporary_path);
        anyhow::bail!("解码器返回了不支持的格式：{extension}");
    }
    let default_name = format!(
        "{}.{}",
        source
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("music"),
        extension
    );
    let output = crate::tasks::runtime::run_io_blocking({
        let extension = extension.clone();
        move || {
            crate::utils::file_picker::pick_save_path_with_filter(
                "Decoded Music",
                &[extension.as_str()],
                &default_name,
            )
        }
    })
    .await
    .map_err(anyhow::Error::msg)?
    .map(PathBuf::from);
    let Some(output) = output else {
        remove_temporary_file(&temporary_path);
        return Ok(None);
    };

    let publish_result = crate::tasks::runtime::run_io_blocking({
        let temporary_path = temporary_path.clone();
        let output = output.clone();
        move || std::fs::copy(&temporary_path, &output).map(|_| ())
    })
    .await
    .map_err(anyhow::Error::msg)?;
    remove_temporary_file(&temporary_path);
    publish_result.map_err(anyhow::Error::from)?;
    Ok(Some(output))
}

fn temporary_output_path(source: &Path) -> PathBuf {
    let identifier = format!(
        "{}-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos()),
        source
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("music")
    );
    crate::utils::file_ops::cache_subdir("ncm-export").join(identifier)
}

fn remove_temporary_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "remove NCM export temporary file failed")
        }
    }
}
