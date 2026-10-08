use crate::core::native_mods::{NativeModEntry, Release};
use crate::ui::components::button::Button;
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::DownloadPageState;
use gpui::*;

#[derive(Default)]
/// Modal-owned release choices and a cancellable request; cleared when the modal or route closes.
pub(crate) struct VersionState {
    releases: Vec<Release>,
    selected: Option<String>,
    loading: bool,
    error: Option<SharedString>,
    request_id: u64,
    task: Option<Task<()>>,
}

impl VersionState {
    pub(crate) fn clear(&mut self) {
        self.task.take();
        self.request_id = self.request_id.wrapping_add(1);
        self.releases.clear();
        self.selected = None;
        self.loading = false;
        self.error = None;
    }

    fn tags(&self, entry: &NativeModEntry, file_name: &str) -> Vec<String> {
        let Some(file) = entry.files.get(file_name) else {
            return Vec::new();
        };
        if !file.url.contains("{{tag}}") {
            return Vec::new();
        }
        self.releases
            .iter()
            .filter(|release| release.supports(&file.url))
            .map(|release| release.tag_name.clone())
            .collect()
    }

    pub(super) fn select_file(&mut self, entry: &NativeModEntry, file_name: &str) {
        let tags = self.tags(entry, file_name);
        if self.selected.as_ref().is_none_or(|tag| !tags.contains(tag)) {
            self.selected = tags.into_iter().next();
        }
    }

    pub(super) fn selected_tag(&self, entry: &NativeModEntry, file_name: &str) -> Option<String> {
        let file = entry.files.get(file_name)?;
        let tag = self.selected.as_ref()?;
        (file.url.contains("{{tag}}")
            && self
                .releases
                .iter()
                .any(|release| &release.tag_name == tag && release.supports(&file.url)))
        .then(|| tag.clone())
    }
}

pub(super) fn load(cx: &mut App) {
    let request = cx.update_global(|state: &mut DownloadPageState, _| {
        let entry = state.native_mod_selected.as_ref()?;
        if !entry
            .files
            .values()
            .any(|file| file.url.contains("{{tag}}"))
        {
            return None;
        }
        state.native_mod_versions.clear();
        state.native_mod_versions.loading = true;
        Some((
            entry.repository.clone(),
            state.native_mod_versions.request_id,
        ))
    });
    let Some((repository, request_id)) = request else {
        return;
    };
    let task = cx.spawn(async move |cx| {
        let result = gpui_tokio::Tokio::spawn_result(cx, async move {
            crate::core::native_mods::releases(&repository)
                .await
                .map_err(anyhow::Error::msg)
        })
        .await;
        if let Err(error) = cx.update_global(|state: &mut DownloadPageState, _| {
            apply_releases(state, request_id, result)
        }) {
            tracing::warn!(%error, "native-mod release UI update failed");
        }
    });
    cx.update_global(|state: &mut DownloadPageState, _| {
        state.native_mod_versions.task = Some(task)
    });
}

fn apply_releases(
    state: &mut DownloadPageState,
    request_id: u64,
    result: anyhow::Result<Vec<Release>>,
) {
    if !state.native_mod_modal_open || state.native_mod_versions.request_id != request_id {
        return;
    }
    let versions = &mut state.native_mod_versions;
    versions.loading = false;
    match result {
        Ok(releases) => {
            versions.releases = releases;
            if let Some(entry) = &state.native_mod_selected {
                versions.select_file(entry, &state.native_mod_selected_file);
            }
        }
        Err(error) => versions.error = Some(error.to_string().into()),
    }
}

pub(super) fn render(colors: &ThemeColors, cx: &App, entry: &NativeModEntry) -> Div {
    let state = cx.global::<DownloadPageState>();
    if !state.native_mod_selected_file.as_ref().is_empty()
        && entry
            .files
            .get(state.native_mod_selected_file.as_ref())
            .is_none_or(|file| !file.url.contains("{{tag}}"))
    {
        return div();
    }
    let versions = &state.native_mod_versions;
    let dropdown = render_dropdown(colors, state, entry);
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(t!("NativeMods.select_version")),
        )
        .child(dropdown)
        .children(versions.error.as_ref().map(|error| {
            div()
                .text_size(px(12.))
                .text_color(colors.danger)
                .child(error.clone())
        }))
        .when(versions.error.is_some(), |div| {
            div.child(
                Button::new("native-mod-versions-retry")
                    .label(t!("NativeMods.reload"))
                    .on_click(|_, _, cx| load(cx)),
            )
        })
}

fn render_dropdown(
    colors: &ThemeColors,
    state: &DownloadPageState,
    entry: &NativeModEntry,
) -> Dropdown {
    let versions = &state.native_mod_versions;
    let tags = versions.tags(entry, &state.native_mod_selected_file);
    let selected_index = versions
        .selected
        .as_ref()
        .and_then(|tag| tags.iter().position(|candidate| candidate == tag))
        .unwrap_or(0);
    let label: SharedString = if versions.loading {
        t!("NativeMods.loading")
    } else {
        tags.get(selected_index)
            .cloned()
            .map(Into::into)
            .unwrap_or_else(|| t!("NativeMods.no_release"))
    };
    let options = tags
        .iter()
        .cloned()
        .map(SharedString::from)
        .map(DropdownOption::from)
        .collect();
    Dropdown::new(
        "native-mod-version-dropdown",
        colors,
        px(260.),
        label,
        options,
        selected_index,
        !versions.loading && !state.native_mod_install_busy && !tags.is_empty(),
        move |index, _, cx| {
            if let Some(tag) = tags.get(index) {
                cx.update_global(|state: &mut DownloadPageState, _| {
                    state.native_mod_versions.selected = Some(tag.clone());
                    state.native_mod_install_error = None;
                });
            }
        },
    )
    .with_height(px(32.))
    .rounded(px(crate::ui::theme::tokens::radius::SM))
}

#[cfg(test)]
mod tests {
    use super::{DownloadPageState, NativeModEntry, VersionState, apply_releases};

    #[test]
    fn changing_files_preserves_only_a_release_with_the_requested_asset() {
        let entry: NativeModEntry = serde_json::from_str(
            r#"{
            "id":"example", "name":"Example", "description":"", "repository":"",
            "author":"", "tags":[], "files": {
                "Mod.dll":{"url":"https://example.com/{{tag}}/Mod.dll", "type":"native.dll"},
                "Other.dll":{"url":"https://example.com/{{tag}}/Other.dll", "type":"native.dll"},
                "Direct.dll":{"url":"https://example.com/Direct.dll", "type":"native.dll"}
            }
        }"#,
        )
        .expect("valid catalog fixture");
        let releases = serde_json::from_str(
            r#"[
            {"tag_name":"v2", "assets":[{"browser_download_url":"https://example.com/v2/Mod.dll"}]},
            {"tag_name":"v1", "assets":[
                {"browser_download_url":"https://example.com/v1/Mod.dll"},
                {"browser_download_url":"https://example.com/v1/Other.dll"}
            ]}
        ]"#,
        )
        .expect("valid releases fixture");
        let mut versions = VersionState {
            releases,
            ..Default::default()
        };
        versions.select_file(&entry, "Mod.dll");
        assert_eq!(
            versions.selected_tag(&entry, "Mod.dll").as_deref(),
            Some("v2")
        );
        versions.selected = Some("v1".into());
        versions.select_file(&entry, "Mod.dll");
        assert_eq!(
            versions.selected_tag(&entry, "Mod.dll").as_deref(),
            Some("v1")
        );
        versions.selected = Some("v2".into());
        versions.select_file(&entry, "Other.dll");
        assert_eq!(
            versions.selected_tag(&entry, "Other.dll").as_deref(),
            Some("v1")
        );
        versions.select_file(&entry, "Direct.dll");
        assert!(versions.selected_tag(&entry, "Direct.dll").is_none());
    }

    #[test]
    fn closed_or_replaced_modal_rejects_old_release_results() {
        let mut state = DownloadPageState::default();
        state.native_mod_modal_open = true;
        let old_request = state.native_mod_versions.request_id;
        state.release_native_mod_state();
        state.native_mod_modal_open = true;
        apply_releases(&mut state, old_request, Err(anyhow::anyhow!("old error")));
        assert!(state.native_mod_versions.error.is_none());
        state.native_mod_modal_open = false;
        let current_request = state.native_mod_versions.request_id;
        apply_releases(
            &mut state,
            current_request,
            Err(anyhow::anyhow!("closed error")),
        );
        assert!(state.native_mod_versions.error.is_none());
    }
}
