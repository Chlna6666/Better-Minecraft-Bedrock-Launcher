use crate::ui::theme::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Default)]
pub struct MarkdownDocument {
    pub blocks: Vec<MarkdownBlock>,
}

#[derive(Debug, Clone)]
pub enum MarkdownBlock {
    Heading {
        level: u8,
        spans: Vec<InlineSpan>,
    },
    Paragraph {
        spans: Vec<InlineSpan>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<InlineSpan>>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Quote {
        spans: Vec<InlineSpan>,
    },
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSpan {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ParseState {
    current_heading: Option<(u8, Vec<InlineSpan>)>,
    current_paragraph: Option<Vec<InlineSpan>>,
    item_stack: Vec<Vec<InlineSpan>>,
    list_items: Vec<Vec<InlineSpan>>,
    list_depth: usize,
    list_ordered: bool,
    blockquote_depth: usize,
    code_block_language: Option<String>,
    code_block_content: String,
    strong_depth: usize,
    emphasis_depth: usize,
    strike_depth: usize,
    active_link: Option<String>,
}

pub fn warm_highlighter_assets() {}

pub fn parse_markdown_document(markdown: &str) -> MarkdownDocument {
    let markdown = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let normalized_markdown = normalize_update_metadata_lines(markdown);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(&normalized_markdown, options);
    let mut state = ParseState::default();
    let mut blocks = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => start_tag(tag, &mut state),
            Event::End(tag_end) => end_tag(tag_end, &mut state, &mut blocks),
            Event::Text(text) => push_text(&text, &mut state, false),
            Event::Code(code) => push_text(&code, &mut state, true),
            Event::SoftBreak | Event::HardBreak => push_text("\n", &mut state, false),
            Event::Rule => blocks.push(MarkdownBlock::Rule),
            _ => {}
        }
    }

    flush_unclosed_state(&mut state, &mut blocks);
    MarkdownDocument { blocks }
}

fn normalize_update_metadata_lines(markdown: &str) -> String {
    let mut normalized = String::with_capacity(markdown.len() + 32);
    for line in markdown.lines() {
        normalized.push_str(line);
        normalized.push('\n');
        if line.starts_with("发布日期：")
            || line.starts_with("范围：")
            || line.starts_with("基准提交：")
        {
            normalized.push('\n');
        }
    }
    normalized
}

pub fn render_markdown_document(
    document: &MarkdownDocument,
    colors: &ThemeColors,
    is_dark: bool,
) -> Div {
    render_markdown_document_limited(document, colors, is_dark, document.blocks.len())
}

pub fn render_markdown_document_limited(
    document: &MarkdownDocument,
    colors: &ThemeColors,
    is_dark: bool,
    max_blocks: usize,
) -> Div {
    let mut column = div().flex().flex_col().gap(px(6.));

    for block in document.blocks.iter().take(max_blocks) {
        column = column.child(render_block(block, colors, is_dark));
    }

    column
}

fn start_tag(tag: Tag<'_>, state: &mut ParseState) {
    match tag {
        Tag::Heading { level, .. } => {
            state.current_heading = Some((level as u8, Vec::new()));
        }
        Tag::Paragraph => {
            if state.current_paragraph.is_none() {
                state.current_paragraph = Some(Vec::new());
            }
        }
        Tag::List(start) => {
            if state.list_depth == 0 {
                state.list_ordered = start.is_some();
                state.list_items.clear();
            }
            state.list_depth += 1;
        }
        Tag::Item => {
            state.item_stack.push(Vec::new());
        }
        Tag::Strong => state.strong_depth += 1,
        Tag::Emphasis => state.emphasis_depth += 1,
        Tag::Strikethrough => state.strike_depth += 1,
        Tag::BlockQuote(_) => state.blockquote_depth += 1,
        Tag::Link { dest_url, .. } => {
            state.active_link = Some(dest_url.to_string());
        }
        Tag::CodeBlock(kind) => {
            state.code_block_content.clear();
            state.code_block_language = Some(match kind {
                CodeBlockKind::Indented => String::new(),
                CodeBlockKind::Fenced(lang) => lang.to_string(),
            });
        }
        _ => {}
    }
}

fn end_tag(tag_end: TagEnd, state: &mut ParseState, blocks: &mut Vec<MarkdownBlock>) {
    match tag_end {
        TagEnd::Heading { .. } => {
            if let Some((level, spans)) = state.current_heading.take()
                && !spans.is_empty()
            {
                blocks.push(MarkdownBlock::Heading { level, spans });
            }
        }
        TagEnd::Paragraph => {
            if let Some(spans) = state.current_paragraph.take()
                && !spans.is_empty()
            {
                if state.blockquote_depth > 0 {
                    blocks.push(MarkdownBlock::Quote { spans });
                } else if state.item_stack.is_empty() {
                    blocks.push(MarkdownBlock::Paragraph { spans });
                } else {
                    append_spans_to_current_item(spans, state);
                }
            }
        }
        TagEnd::Item => {
            if let Some(spans) = state.item_stack.pop()
                && !spans.is_empty()
            {
                if let Some(parent) = state.item_stack.last_mut() {
                    if !parent.is_empty() {
                        parent.push(InlineSpan {
                            text: "\n".to_string(),
                            style: InlineStyle::default(),
                        });
                    }
                    parent.push(InlineSpan {
                        text: "  • ".to_string(),
                        style: InlineStyle::default(),
                    });
                    parent.extend(spans);
                } else {
                    state.list_items.push(spans);
                }
            }
        }
        TagEnd::List(_) => {
            if state.list_depth > 0 {
                state.list_depth -= 1;
            }
            if state.list_depth == 0 {
                if !state.list_items.is_empty() {
                    blocks.push(MarkdownBlock::List {
                        ordered: state.list_ordered,
                        items: std::mem::take(&mut state.list_items),
                    });
                }
            }
        }
        TagEnd::Strong => state.strong_depth = state.strong_depth.saturating_sub(1),
        TagEnd::Emphasis => state.emphasis_depth = state.emphasis_depth.saturating_sub(1),
        TagEnd::Strikethrough => state.strike_depth = state.strike_depth.saturating_sub(1),
        TagEnd::BlockQuote(_) => {
            state.blockquote_depth = state.blockquote_depth.saturating_sub(1);
        }
        TagEnd::Link => {
            state.active_link = None;
        }
        TagEnd::CodeBlock => {
            let language = state.code_block_language.take();
            let code = std::mem::take(&mut state.code_block_content);
            if !code.trim().is_empty() {
                let language = language.and_then(|lang| {
                    let trimmed = lang.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                });
                blocks.push(MarkdownBlock::CodeBlock { language, code });
            }
        }
        _ => {}
    }
}

fn push_text(text: &str, state: &mut ParseState, is_inline_code: bool) {
    if state.code_block_language.is_some() {
        state.code_block_content.push_str(text);
        return;
    }

    if text.is_empty() {
        return;
    }

    let style = InlineStyle {
        bold: state.strong_depth > 0,
        italic: state.emphasis_depth > 0,
        strike: state.strike_depth > 0,
        code: is_inline_code,
        link: state.active_link.clone(),
    };

    if let Some(spans) = active_spans_mut(state) {
        push_span(spans, text, style);
    }
}

fn push_span(spans: &mut Vec<InlineSpan>, text: &str, style: InlineStyle) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.text.push_str(text);
        return;
    }

    spans.push(InlineSpan {
        text: text.to_string(),
        style,
    });
}

fn append_spans_to_current_item(spans: Vec<InlineSpan>, state: &mut ParseState) {
    if let Some(item_spans) = state.item_stack.last_mut() {
        if !item_spans.is_empty() {
            item_spans.push(InlineSpan {
                text: "\n".to_string(),
                style: InlineStyle::default(),
            });
        }
        item_spans.extend(spans);
    }
}

fn active_spans_mut(state: &mut ParseState) -> Option<&mut Vec<InlineSpan>> {
    if let Some((_, spans)) = state.current_heading.as_mut() {
        return Some(spans);
    }
    if let Some(spans) = state.item_stack.last_mut() {
        return Some(spans);
    }
    state.current_paragraph.as_mut()
}

fn flush_unclosed_state(state: &mut ParseState, blocks: &mut Vec<MarkdownBlock>) {
    if let Some((level, spans)) = state.current_heading.take()
        && !spans.is_empty()
    {
        blocks.push(MarkdownBlock::Heading { level, spans });
    }

    while let Some(spans) = state.item_stack.pop() {
        if !spans.is_empty() {
            state.list_items.push(spans);
        }
    }

    if !state.list_items.is_empty() {
        blocks.push(MarkdownBlock::List {
            ordered: state.list_ordered,
            items: std::mem::take(&mut state.list_items),
        });
    }

    if let Some(spans) = state.current_paragraph.take()
        && !spans.is_empty()
    {
        blocks.push(MarkdownBlock::Paragraph { spans });
    }

    if !state.code_block_content.trim().is_empty() {
        let language = state.code_block_language.take();
        let code = std::mem::take(&mut state.code_block_content);
        blocks.push(MarkdownBlock::CodeBlock { language, code });
    }
}

fn render_block(block: &MarkdownBlock, colors: &ThemeColors, is_dark: bool) -> AnyElement {
    match block {
        MarkdownBlock::Heading { level, spans } => {
            let (font_size, line_height, padding_bottom, with_divider) = match *level {
                1 => (px(21.), px(30.), px(8.), true),
                2 => (px(18.), px(27.), px(7.), true),
                _ => (px(16.), px(24.), px(0.), false),
            };

            div()
                .w_full()
                .pt(px(2.))
                .pb(padding_bottom)
                .when(with_divider, |this| {
                    this.border_b_1().border_color(colors.border)
                })
                .child(render_inline_styled_text(
                    spans,
                    colors,
                    font_size,
                    line_height,
                    true,
                    is_dark,
                    true,
                ))
                .into_any_element()
        }
        MarkdownBlock::Paragraph { spans } => div()
            .w_full()
            .child(render_inline_styled_text(
                spans,
                colors,
                px(14.),
                px(22.),
                false,
                is_dark,
                true,
            ))
            .into_any_element(),
        MarkdownBlock::Quote { spans } => div()
            .w_full()
            .pl(px(10.))
            .border_l_2()
            .border_color(colors.border)
            .child(render_inline_styled_text(
                spans,
                colors,
                px(14.),
                px(22.),
                false,
                is_dark,
                true,
            ))
            .into_any_element(),
        MarkdownBlock::List { ordered, items } => {
            let mut list = div().flex().flex_col().gap(px(4.));
            for (index, item_spans) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}.", index + 1)
                } else {
                    "•".to_string()
                };

                list = list.child(
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(16.))
                                .text_size(px(14.))
                                .line_height(px(22.))
                                .text_color(colors.text_secondary)
                                .child(marker),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(render_inline_styled_text(
                                    item_spans,
                                    colors,
                                    px(14.),
                                    px(22.),
                                    false,
                                    is_dark,
                                    true,
                                )),
                        ),
                );
            }
            list.into_any_element()
        }
        MarkdownBlock::CodeBlock { language, code } => {
            render_code_block(language.as_deref(), code, colors, is_dark).into_any_element()
        }
        MarkdownBlock::Rule => div()
            .h(px(1.))
            .w_full()
            .bg(colors.border)
            .into_any_element(),
    }
}

fn render_inline_styled_text(
    spans: &[InlineSpan],
    colors: &ThemeColors,
    font_size: Pixels,
    line_height: Pixels,
    is_heading: bool,
    is_dark: bool,
    allow_inline_code_pills: bool,
) -> AnyElement {
    if spans.is_empty() {
        return div().into_any_element();
    }

    let code_background = inline_code_background(is_dark);
    let code_background_padding = TextBackgroundPadding {
        top: px(1.),
        right: px(4.),
        bottom: px(1.),
        left: px(4.),
    };
    let mut combined_text = String::new();
    let mut highlights = Vec::new();

    for span in spans {
        let mut text = span.text.clone();
        if !span.style.code {
            text = soft_wrap_markdown_text(&text);
        }
        if text.is_empty() {
            continue;
        }

        let mut color = colors.text_primary;
        if span.style.link.is_some() {
            color = colors.accent;
        }
        if span.style.code {
            color = colors.text_primary;
        }

        let mut font_weight = if is_heading {
            FontWeight::EXTRA_BOLD
        } else {
            FontWeight::NORMAL
        };
        if span.style.bold {
            font_weight = FontWeight::BOLD;
        }

        let underline = span.style.link.as_ref().map(|_| UnderlineStyle {
            thickness: px(1.),
            color: Some(colors.accent),
            wavy: false,
        });

        let strikethrough = span.style.strike.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(color),
        });

        let range_start = combined_text.len();
        combined_text.push_str(&text);
        let range_end = combined_text.len();
        highlights.push((
            range_start..range_end,
            HighlightStyle {
                color: Some(color),
                font_weight: Some(font_weight),
                font_style: span.style.italic.then_some(FontStyle::Italic),
                background_color: (span.style.code && allow_inline_code_pills)
                    .then_some(code_background),
                background_corner_radius: (span.style.code && allow_inline_code_pills)
                    .then_some(px(4.)),
                background_padding: (span.style.code && allow_inline_code_pills)
                    .then_some(code_background_padding),
                underline,
                strikethrough,
                fade_out: None,
            },
        ));
    }

    if combined_text.is_empty() {
        return div().into_any_element();
    }

    div()
        .w_full()
        .text_size(font_size)
        .line_height(line_height)
        .whitespace_normal()
        .child(StyledText::new(SharedString::from(combined_text)).with_highlights(highlights))
        .into_any_element()
}

fn inline_code_background(is_dark: bool) -> Hsla {
    if is_dark {
        Hsla {
            a: 0.36,
            ..rgb(0x6e7681).into()
        }
    } else {
        Hsla {
            a: 0.28,
            ..rgb(0xafb8c1).into()
        }
    }
}

fn render_code_block(
    language: Option<&str>,
    code: &str,
    colors: &ThemeColors,
    is_dark: bool,
) -> Div {
    let mut lines = div().flex().flex_col().gap(px(2.));

    for line in code.lines() {
        let line = if line.is_empty() { " " } else { line };
        lines = lines.child(
            div()
                .w_full()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(if is_dark {
                    colors.text_secondary
                } else {
                    colors.text_primary
                })
                .whitespace_normal()
                .child(line.to_string()),
        );
    }

    let language_label = language.unwrap_or("text").trim();

    div()
        .w_full()
        .mt(px(2.))
        .rounded(px(8.))
        .bg(if is_dark {
            hsla(0.60, 0.09, 0.22, 1.0)
        } else {
            hsla(0.60, 0.08, 0.97, 1.0)
        })
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .px(px(10.))
                .py(px(6.))
                .bg(if is_dark {
                    hsla(0.60, 0.08, 0.27, 1.0)
                } else {
                    hsla(0.60, 0.06, 0.94, 1.0)
                })
                .border_b_1()
                .border_color(colors.border)
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_muted)
                .child(language_label.to_string()),
        )
        .child(div().w_full().px(px(10.)).py(px(8.)).child(lines))
}

fn soft_wrap_markdown_text(input: &str) -> String {
    const ZERO_WIDTH_SPACE: char = '\u{200B}';
    let mut wrapped = String::with_capacity(input.len() + input.len() / 8);
    let mut run_without_space = 0usize;

    for ch in input.chars() {
        if ch.is_whitespace() {
            run_without_space = 0;
        } else {
            run_without_space += 1;
        }

        wrapped.push(ch);

        if matches!(
            ch,
            '/' | '\\' | '.' | '-' | '_' | ':' | '?' | '&' | '=' | '#' | '%'
        ) {
            wrapped.push(ZERO_WIDTH_SPACE);
            run_without_space = 0;
        } else if run_without_space >= 28 {
            wrapped.push(ZERO_WIDTH_SPACE);
            run_without_space = 0;
        }
    }

    wrapped
}

#[cfg(test)]
mod tests {
    use crate::ui::components::markdown_renderer::{MarkdownBlock, parse_markdown_document};

    #[test]
    fn parses_complete_markdown_list_items() {
        let markdown = (1..=64)
            .map(|index| format!("- item {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let document = parse_markdown_document(&markdown);

        let list_item_count = document
            .blocks
            .iter()
            .filter_map(|block| match block {
                MarkdownBlock::List { items, .. } => Some(items.len()),
                _ => None,
            })
            .sum::<usize>();

        assert_eq!(list_item_count, 64);
    }
}
