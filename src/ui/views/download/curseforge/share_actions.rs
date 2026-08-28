use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::views::download::common::{format_count, format_date_ymd};
use crate::ui::views::download::state::CurseForgeModEntry;
use gpui::*;

pub(crate) fn handle_clipboard_share_paste(cx: &mut App) {
    let text = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    handle_curseforge_share_text(&text, cx);
}

pub(super) fn curseforge_project_url(mod_entry: &CurseForgeModEntry) -> SharedString {
    SharedString::from(format!(
        "https://www.curseforge.com/projects/{}",
        mod_entry.id
    ))
}

pub(super) fn copy_curseforge_link(mod_entry: &CurseForgeModEntry, cx: &mut App) {
    write_text_to_clipboard(curseforge_project_url(mod_entry), cx);
    toast::push(cx, t!("CurseForge.share_link_copied"));
}

pub(super) fn copy_curseforge_share_text(mod_entry: &CurseForgeModEntry, cx: &mut App) {
    let url = curseforge_project_url(mod_entry);
    let name = mod_entry.name.to_string();
    let id = mod_entry.id.to_string();
    let content = t!(
        "CurseForge.share_text",
        name = &name,
        url = url.as_ref(),
        id = &id
    );
    write_text_to_clipboard(content, cx);
    toast::push(cx, t!("CurseForge.share_text_copied"));
}

pub(super) fn copy_curseforge_analysis(
    mod_entry: &CurseForgeModEntry,
    categories: &[SharedString],
    cx: &mut App,
) {
    let authors = if mod_entry.author_names.is_empty() {
        t!("CurseForge.unknown_author").to_string()
    } else {
        mod_entry
            .author_names
            .iter()
            .map(|name| name.as_ref())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let category_text = if categories.is_empty() {
        t!("CurseForge.uncategorized").to_string()
    } else {
        categories
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let name = mod_entry.name.to_string();
    let updated = format_date_ymd(mod_entry.date_modified.as_ref());
    let downloads = format_count(mod_entry.download_count);
    let url = curseforge_project_url(mod_entry);
    let content = t!(
        "CurseForge.analysis_text",
        name = &name,
        authors = &authors,
        updated = updated.as_ref(),
        downloads = downloads.as_ref(),
        categories = &category_text,
        url = url.as_ref()
    );
    write_text_to_clipboard(content, cx);
    toast::push(cx, t!("CurseForge.analysis_copied"));
}

pub(super) fn handle_curseforge_share_text(text: &str, cx: &mut App) {
    let Some(mod_id) = parse_shared_curseforge_id(text) else {
        toast::error(cx, t!("CurseForge.share_invalid"));
        return;
    };

    toast::push(
        cx,
        t!("CurseForge.share_id_detected", id = &mod_id.to_string()),
    );
    super::modals::open_curseforge_mod_page(mod_id, cx);
}

fn write_text_to_clipboard(message: impl Into<SharedString>, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(message.into().to_string()));
}

fn parse_shared_curseforge_id(text: &str) -> Option<i32> {
    for line in text.lines() {
        let normalized = line.trim().replace('\u{ff1a}', ":").replace('\u{200b}', "");
        let upper = normalized.to_uppercase();
        if let Some(index) = upper.find("ID:") {
            let tail = &normalized[index + 3..];
            if let Some(number) = read_leading_int(tail.trim()) {
                return Some(number);
            }
        }
    }

    None
}

fn read_leading_int(text: &str) -> Option<i32> {
    let mut buffer = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            buffer.push(character);
        } else {
            break;
        }
    }
    if buffer.is_empty() {
        return None;
    }
    let parsed = buffer.parse::<i32>().ok()?;
    if parsed > 0 { Some(parsed) } else { None }
}
