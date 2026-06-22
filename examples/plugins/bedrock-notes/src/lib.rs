use bmcbl_plugin_api::prelude::*;

const PLUGIN: PluginMetadata = plugin_metadata!();
const PATCH_NOTES_LIST_URL: &str = "https://launchercontent.mojang.com/v2/bedrockPatchNotes.json";
const PATCH_NOTES_BASE_URL: &str = "https://launchercontent.mojang.com";
const CACHE_TTL_SECONDS: u32 = 30 * 60;
const MAX_PATCH_NOTES_BYTES: u32 = 512 * 1024;
const HOME_PAGE_ID: &str = "home";
const LIST_PAGE_ID: &str = "main";
const DETAIL_PAGE_ID: &str = "detail";
const SESSION_SELECTED_NOTE_PATH: &str = "selected-note-path";

struct BedrockNotesPlugin;

plugin_actions! {
    pub enum NotesAction {
        OpenList = "open-list",
        OpenDetail = "open-detail",
    }
}

#[derive(Clone, Debug, Default)]
struct PatchNote {
    title: String,
    version: String,
    patch_note_type: String,
    date: String,
    short_text: String,
    body_html: Option<String>,
    content_path: Option<String>,
    image_url: Option<String>,
}

#[bmcbl_plugin]
impl Plugin for BedrockNotesPlugin {
    fn init(_context: PluginContext) -> PluginResult<Vec<Registration>> {
        Ok(registrations! {
            page HOME_PAGE_ID, title = tr!("notes.home_title");
            page LIST_PAGE_ID, title = tr!("notes.title");
            page DETAIL_PAGE_ID, title = tr!("notes.detail_title");
            injection InjectionSlot::HomeSidebar, page = "/", priority = 40, layout =
                InjectionLayout::sidebar()
                    .width(304)
                    .min_width(248)
                    .max_width(328)
                    .max_height(320)
                    .compact_behavior(CompactBehavior::Scroll);
        })
    }

    fn handle_event(event: HostEvent) -> PluginResult<()> {
        if event.action_is(NotesAction::OpenList.as_str()) {
            PLUGIN
                .modal(LIST_PAGE_ID)
                .title(tr!("notes.window_title"))
                .size(560, 620)
                .open(PLUGIN)?;
            return Ok(());
        }

        if event.action_is(NotesAction::OpenDetail.as_str())
            && let HostEventKind::Action(action) = &event.kind
            && let Some(content_path) = action.value.clone()
        {
            session_set(SESSION_SELECTED_NOTE_PATH, Some(content_path))?;
            PLUGIN
                .modal(DETAIL_PAGE_ID)
                .title(tr!("notes.detail_window_title"))
                .size(600, 620)
                .open(PLUGIN)?;
        }

        Ok(())
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree> {
        let response = http_get_text(
            PATCH_NOTES_LIST_URL,
            CACHE_TTL_SECONDS,
            MAX_PATCH_NOTES_BYTES,
        )?;
        let notes = response
            .body
            .as_deref()
            .map(parse_patch_notes)
            .unwrap_or_default();

        let view = match request.page_id.as_str() {
            HOME_PAGE_ID => home_panel(response.state, response.error.as_deref(), &notes),
            LIST_PAGE_ID => list_page(response.state, response.error.as_deref(), &notes),
            DETAIL_PAGE_ID => detail_page(response.state, response.error.as_deref(), &notes),
            _ => list_page(response.state, response.error.as_deref(), &notes),
        };
        Ok(view.finish())
    }

    fn render_injection(_request: InjectionRequest) -> PluginResult<Option<ViewTree>> {
        let response = http_get_text(
            PATCH_NOTES_LIST_URL,
            CACHE_TTL_SECONDS,
            MAX_PATCH_NOTES_BYTES,
        )?;
        let notes = response
            .body
            .as_deref()
            .map(parse_patch_notes)
            .unwrap_or_default();
        Ok(Some(
            home_panel(response.state, response.error.as_deref(), &notes).finish(),
        ))
    }
}

fn home_panel(state: HttpCacheState, error: Option<&str>, notes: &[PatchNote]) -> View {
    let mut root = base_panel(tr!("notes.home_title"), state, true);
    if let Some(note) = notes.first() {
        root = root.child(home_summary(note));
    } else {
        root = root.child(status_message(state, error));
    }
    root.into()
}

fn list_page(state: HttpCacheState, error: Option<&str>, notes: &[PatchNote]) -> View {
    let mut root = page_panel(tr!("notes.title"), state);
    if notes.is_empty() {
        return root.child(status_message(state, error)).into();
    }

    for note in notes.iter().take(12) {
        root = root.child(list_row(note));
    }
    root.into()
}

fn detail_page(state: HttpCacheState, error: Option<&str>, notes: &[PatchNote]) -> View {
    let mut root = page_panel(tr!("notes.detail_title"), state);
    let Some(content_path) = session_get(SESSION_SELECTED_NOTE_PATH).ok().flatten() else {
        return root.child(text(tr!("notes.detail_empty"))).into();
    };

    let note = notes
        .iter()
        .find(|note| note.content_path.as_deref() == Some(content_path.as_str()))
        .cloned()
        .unwrap_or_default();
    let detail_url = detail_url_for(&content_path);
    let detail_response = match http_get_text(&detail_url, CACHE_TTL_SECONDS, MAX_PATCH_NOTES_BYTES)
    {
        Ok(detail) => detail,
        Err(host_error) => {
            return root
                .child(text(tr!("notes.error", "error" => host_error.message)))
                .into();
        }
    };

    let detail_note = detail_response
        .body
        .as_deref()
        .map(parse_patch_note_detail)
        .unwrap_or(note);

    root = root
        .child(title(detail_title(&detail_note)))
        .child(meta_row(&detail_note));

    if let Some(image_url) = detail_note.image_url.as_ref() {
        root = root.child(image_with_options(
            image_url.clone(),
            detail_note.title.clone(),
            news_image_options(260, 220, 300, 8, None, ImageFit::Cover),
        ));
    }

    root = root.child(text(detail_note.short_text.clone()));

    if let Some(body_html) = detail_note.body_html.as_ref() {
        root = root.child(text(summarize_html(body_html)));
    }

    if matches!(detail_response.state, HttpCacheState::Error) {
        root = root.child(status_message(detail_response.state, error));
    }

    root.into()
}

fn base_panel(title_text: String, state: HttpCacheState, compact: bool) -> Container {
    View::column()
        .padding(if compact { 0 } else { 10 })
        .gap(if compact { 8 } else { 8 })
        .child(
            View::row()
                .padding(0)
                .gap(10)
                .align(Align::Center)
                .child(status_badge(state))
                .child(title(title_text))
                .finish_view(),
        )
}

fn page_panel(title_text: String, state: HttpCacheState) -> Container {
    View::column()
        .padding(4)
        .gap(14)
        .child(
            View::row()
                .padding(0)
                .gap(10)
                .align(Align::Center)
                .child(status_badge(state))
                .child(title(title_text))
                .finish_view(),
        )
}

fn home_summary(note: &PatchNote) -> View {
    let mut root = View::column().padding(0).gap(7);

    if let Some(image_url) = note.image_url.as_ref() {
        root = root.child(image_with_options(
            image_url.clone(),
            note.title.clone(),
            news_image_options(128, 116, 136, 6, None, ImageFit::Cover),
        ));
    }

    let action_row = View::row()
        .padding(0)
        .gap(6)
        .align(Align::Center)
        .child(action_button(
            tr!("notes.read_more"),
            NotesAction::OpenDetail.as_str(),
            Some(note.content_path.clone().unwrap_or_default()),
        ))
        .child(action_button(tr!("notes.more"), NotesAction::OpenList.as_str(), None))
        .finish_view();

    root.child(title(truncate(detail_title(note), 38)))
        .child(compact_meta_row(note))
        .child(text(truncate(note.short_text.clone(), 56)))
        .child(action_row)
        .into()
}

fn list_row(note: &PatchNote) -> View {
    let mut row = View::column()
        .padding(0)
        .gap(10)
        .full_width()
        .corner_radius(8);

    if let Some(image_url) = note.image_url.as_ref() {
        row = row.child(image_with_options(
            image_url.clone(),
            note.title.clone(),
            news_image_options(170, 142, 200, 6, None, ImageFit::Cover),
        ));
    }

    row.child(title(truncate(detail_title(note), 74)))
        .child(compact_meta_row(note))
        .child(text(truncate(note.short_text.clone(), 150)))
        .child(action_button(
            tr!("notes.read_more"),
            NotesAction::OpenDetail.as_str(),
            Some(note.content_path.clone().unwrap_or_default()),
        ))
        .into()
}

fn action_button(label: String, action_id: &'static str, action_value: Option<String>) -> View {
    let style = ViewStyle {
        color: Some(ThemeToken::Accent),
        text_size: TextSizeToken::Small,
        emphasis: true,
        corner_radius: Some(6),
        ..default_style()
    };
    let node = if let Some(action_value) = action_value {
        button_with_value(label, action_id, action_value)
    } else {
        button(label, action_id)
    };
    let ViewNode::Button(mut button) = node else {
        return View::node(node);
    };
    button.style = style;
    View::node(ViewNode::Button(button))
}

fn compact_meta_row(note: &PatchNote) -> View {
    View::row()
        .padding(0)
        .gap(7)
        .align(Align::Center)
        .child(text(format_date(&note.date)))
        .child(text(note.version.clone()))
        .finish_view()
}

fn status_badge(state: HttpCacheState) -> View {
    let background = match state {
        HttpCacheState::Fresh => ThemeToken::Accent,
        HttpCacheState::Loading | HttpCacheState::Stale => ThemeToken::Surface,
        HttpCacheState::Error => ThemeToken::Danger,
    };
    let text_color = match state {
        HttpCacheState::Fresh | HttpCacheState::Error => ThemeToken::Surface,
        HttpCacheState::Loading | HttpCacheState::Stale => ThemeToken::SecondaryText,
    };
    let style = ViewStyle {
        background: Some(background),
        color: Some(text_color),
        text_size: TextSizeToken::Small,
        emphasis: true,
        ..default_style()
    };
    View::badge_with_style(status_label(state), style)
}

fn news_image_options(
    height: u16,
    min_height: u16,
    max_height: u16,
    corner_radius: u16,
    caption: Option<String>,
    fit: ImageFit,
) -> ImageOptions {
    let mut options = ImageOptions::new()
        .height(height)
        .min_height(min_height)
        .max_height(max_height)
        .aspect_ratio(16, 9)
        .fit(fit)
        .corner_radius(corner_radius)
        .placeholder(tr!("notes.image_loading"))
        .fallback(tr!("notes.image_unavailable"));
    if let Some(caption) = caption {
        options = options.caption(caption);
    }
    options
}

fn meta_row(note: &PatchNote) -> View {
    View::row()
        .padding(0)
        .gap(12)
        .align(Align::Center)
        .child(text(note.version.clone()))
        .child(text(clean_patch_type(&note.patch_note_type)))
        .child(text(format_date(&note.date)))
        .finish_view()
}

fn detail_title(note: &PatchNote) -> String {
    if current_locale().starts_with("zh") {
        return note
            .title
            .replace("Minecraft: Bedrock Edition", "Minecraft Bedrock");
    }
    note.title.clone()
}

fn clean_patch_type(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return tr!("notes.type.unknown");
    }
    match value {
        "release" | "Release" => tr!("notes.type.release"),
        "preview" | "Preview" => tr!("notes.type.preview"),
        "beta" | "Beta" => tr!("notes.type.beta"),
        other => other.to_string(),
    }
}

fn status_message(state: HttpCacheState, error: Option<&str>) -> View {
    match state {
        HttpCacheState::Loading => text(tr!("notes.loading")).into(),
        HttpCacheState::Error => {
            text(tr!("notes.error", "error" => error.unwrap_or("unknown"))).into()
        }
        HttpCacheState::Fresh | HttpCacheState::Stale => text(tr!("notes.empty")).into(),
    }
}

fn status_label(state: HttpCacheState) -> String {
    match state {
        HttpCacheState::Loading => tr!("notes.status.loading"),
        HttpCacheState::Fresh => tr!("notes.status.fresh"),
        HttpCacheState::Stale => tr!("notes.status.stale"),
        HttpCacheState::Error => tr!("notes.status.error"),
    }
}

fn detail_url_for(content_path: &str) -> String {
    let path = content_path.trim_start_matches('/');
    if path.starts_with("v2/") {
        format!("{PATCH_NOTES_BASE_URL}/{path}")
    } else {
        format!("{PATCH_NOTES_BASE_URL}/v2/{path}")
    }
}

fn parse_patch_notes(json: &str) -> Vec<PatchNote> {
    let Some(entries_start) = json.find("\"entries\"") else {
        return Vec::new();
    };
    let Some(array_start) = json[entries_start..].find('[') else {
        return Vec::new();
    };
    let mut cursor = &json[entries_start + array_start + 1..];
    let mut notes = Vec::new();

    while let Some(start) = cursor.find('{') {
        cursor = &cursor[start..];
        let Some(entry_end) = find_matching_brace(cursor) else {
            break;
        };
        let entry = &cursor[..=entry_end];
        let note = PatchNote {
            title: decode_mojang_text(&json_string_field(entry, "title")),
            version: decode_mojang_text(&json_string_field(entry, "version")),
            patch_note_type: decode_mojang_text(&json_string_field(entry, "patchNoteType")),
            date: decode_mojang_text(&json_string_field(entry, "date")),
            short_text: truncate(
                decode_mojang_text(&json_string_field(entry, "shortText")),
                460,
            ),
            body_html: None,
            content_path: option_string(json_string_field(entry, "contentPath")),
            image_url: json_nested_string_field(entry, &["image", "url"])
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(resolve_image_url),
        };
        if !note.title.is_empty() {
            notes.push(note);
        }
        if notes.len() >= 12 {
            break;
        }
        cursor = &cursor[entry_end + 1..];
    }

    notes
}

fn parse_patch_note_detail(json: &str) -> PatchNote {
    PatchNote {
        title: decode_mojang_text(&json_string_field(json, "title")),
        version: decode_mojang_text(&json_string_field(json, "version")),
        patch_note_type: decode_mojang_text(&json_string_field(json, "patchNoteType")),
        date: decode_mojang_text(&json_string_field(json, "date")),
        short_text: truncate(
            decode_mojang_text(&json_string_field(json, "shortText")),
            460,
        ),
        body_html: option_string(decode_mojang_text(&json_string_field(json, "body"))),
        content_path: option_string(json_string_field(json, "contentPath")),
        image_url: json_nested_string_field(json, &["image", "url"])
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(resolve_image_url),
    }
}

fn resolve_image_url(url: String) -> String {
    if url.starts_with("https://") {
        return url;
    }
    let mut resolved = String::from(PATCH_NOTES_BASE_URL);
    if !url.starts_with('/') {
        resolved.push('/');
    }
    resolved.push_str(&url);
    resolved
}

fn json_nested_string_field(entry: &str, path: &[&str]) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    if path.len() == 1 {
        return option_string(json_string_field(entry, path[0]));
    }

    let object_needle = {
        let mut value = String::from("\"");
        value.push_str(path[0]);
        value.push_str("\":{");
        value
    };
    let start = entry.find(&object_needle)?;
    let object_start = start + object_needle.len() - 1;
    let object_slice = &entry[object_start..];
    let end = find_matching_brace(object_slice)?;
    let nested = &object_slice[..=end];
    json_nested_string_field(nested, &path[1..])
}

fn json_string_field(entry: &str, key: &str) -> String {
    let needle = {
        let mut value = String::from("\"");
        value.push_str(key);
        value.push_str("\":\"");
        value
    };
    let Some(start) = entry.find(&needle) else {
        return String::new();
    };
    let value_start = start + needle.len();
    let mut value = String::new();
    let mut escaped = false;
    for character in entry[value_start..].chars() {
        if escaped {
            value.push(match character {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '"' {
            break;
        }
        value.push(character);
    }
    value
}

fn find_matching_brace(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
                continue;
            }
            if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn decode_mojang_text(value: &str) -> String {
    value
        .replace("Â ", " ")
        .replace("â", "-")
        .replace("â", "'")
        .replace("â", "\"")
        .replace("â", "\"")
        .replace("â¦", "...")
}

fn summarize_html(body_html: &str) -> String {
    let mut text = String::with_capacity(body_html.len());
    let mut in_tag = false;
    for character in body_html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push('\n');
            }
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    truncate(
        text.replace("&nbsp;", " ")
            .replace("&#x26;", "&")
            .replace("&amp;", "&")
            .replace("&quot;", "\""),
        2200,
    )
}

fn format_date(date: &str) -> String {
    date.chars().take(10).collect()
}

fn option_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn truncate(mut value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    value = value.chars().take(max_chars).collect();
    value.push_str("...");
    value
}
