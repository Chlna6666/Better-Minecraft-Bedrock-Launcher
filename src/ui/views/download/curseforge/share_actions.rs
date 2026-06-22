use crate::ui::components::toast;
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
    toast::push(cx, SharedString::from("已复制项目链接"));
}

pub(super) fn copy_curseforge_share_text(mod_entry: &CurseForgeModEntry, cx: &mut App) {
    let url = curseforge_project_url(mod_entry);
    let content = format!(
        "你的好友向你推荐了一个资源【{}】\n地址：{}\n前往 BMCBL 后粘贴即可打开该资源\nID: {}",
        mod_entry.name, url, mod_entry.id
    );
    write_text_to_clipboard(SharedString::from(content), cx);
    toast::push(cx, SharedString::from("已复制分享文本"));
}

pub(super) fn copy_curseforge_analysis(
    mod_entry: &CurseForgeModEntry,
    categories: &[SharedString],
    cx: &mut App,
) {
    let authors = if mod_entry.author_names.is_empty() {
        "未知作者".to_string()
    } else {
        mod_entry
            .author_names
            .iter()
            .map(|name| name.as_ref())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let category_text = if categories.is_empty() {
        "未分类".to_string()
    } else {
        categories
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let content = format!(
        "名称: {}\n作者: {}\n更新时间: {}\n下载量: {}\n分类: {}\n链接: {}",
        mod_entry.name,
        authors,
        format_date_ymd(mod_entry.date_modified.as_ref()),
        format_count(mod_entry.download_count),
        category_text,
        curseforge_project_url(mod_entry)
    );
    write_text_to_clipboard(SharedString::from(content), cx);
    toast::push(cx, SharedString::from("已复制资源分析"));
}

pub(super) fn handle_curseforge_share_text(text: &str, cx: &mut App) {
    let Some(mod_id) = parse_shared_curseforge_id(text) else {
        toast::error(
            cx,
            SharedString::from("剪贴板内容无法识别，需要包含 `ID:` 字段"),
        );
        return;
    };

    toast::push(
        cx,
        SharedString::from(format!("已识别 CurseForge ID: {mod_id}")),
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
