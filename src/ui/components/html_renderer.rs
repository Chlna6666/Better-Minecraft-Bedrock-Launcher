use crate::ui::components::icon::themed_icon;
use crate::ui::theme::ThemeColors;
use ego_tree::NodeRef;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use scraper::{ElementRef, Html, Node};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct HtmlDocument {
    pub blocks: Vec<HtmlBlock>,
    pub image_urls: Vec<SharedString>,
    pub plain_text_lines: Vec<SharedString>,
}

#[derive(Debug, Clone)]
pub enum HtmlBlock {
    Heading {
        level: u8,
        spans: Vec<HtmlInline>,
    },
    Paragraph {
        spans: Vec<HtmlInline>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<HtmlInline>>,
    },
    Quote {
        spans: Vec<HtmlInline>,
    },
    Image {
        src: SharedString,
        alt: SharedString,
    },
    Video {
        src: SharedString,
        title: SharedString,
        provider: SharedString,
    },
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlInline {
    pub text: String,
    pub style: HtmlInlineStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HtmlInlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
    pub color: Option<Hsla>,
}

pub fn parse_html_document(html: &str) -> HtmlDocument {
    let html = html.trim();
    if html.is_empty() {
        return HtmlDocument::default();
    }

    let fragment = Html::parse_fragment(html);
    let root = fragment.tree.root();
    let mut blocks = Vec::new();
    let mut pending_paragraph = Vec::new();
    let mut image_urls = Vec::new();

    parse_nodes(
        root,
        &HtmlInlineStyle::default(),
        &mut blocks,
        &mut pending_paragraph,
        &mut image_urls,
    );
    flush_pending_paragraph(&mut blocks, &mut pending_paragraph);

    let plain_text_lines = collect_plain_text_lines(&blocks);

    HtmlDocument {
        blocks,
        image_urls,
        plain_text_lines,
    }
}

pub fn render_html_document(
    document: &HtmlDocument,
    colors: &ThemeColors,
    image_cache: Option<&HashMap<SharedString, Arc<RenderImage>>>,
) -> Div {
    let mut column = div().flex().flex_col().gap(px(14.));

    for block in &document.blocks {
        column = column.child(render_block(block, colors, image_cache));
    }

    column
}

fn parse_nodes(
    parent: NodeRef<'_, Node>,
    inherited_style: &HtmlInlineStyle,
    blocks: &mut Vec<HtmlBlock>,
    pending_paragraph: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    for child in parent.children() {
        match child.value() {
            Node::Text(text) => {
                push_inline_text(pending_paragraph, &text.text, inherited_style.clone());
            }
            Node::Element(_) => {
                if let Some(element_ref) = ElementRef::wrap(child) {
                    parse_element(
                        element_ref,
                        inherited_style,
                        blocks,
                        pending_paragraph,
                        image_urls,
                    );
                }
            }
            _ => {}
        }
    }
}

fn parse_element(
    element_ref: ElementRef<'_>,
    inherited_style: &HtmlInlineStyle,
    blocks: &mut Vec<HtmlBlock>,
    pending_paragraph: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    let tag_name = element_ref.value().name();
    let inline_style = merge_inline_style(tag_name, inherited_style, &element_ref);

    match tag_name {
        "br" => push_inline_text(pending_paragraph, "\n", inline_style),
        "hr" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            blocks.push(HtmlBlock::Rule);
        }
        "img" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            if let Some(src) = element_ref.value().attr("src") {
                let src = SharedString::from(src.trim().to_string());
                if !src.trim().is_empty() {
                    image_urls.push(src.clone());
                    let alt = element_ref.value().attr("alt").unwrap_or_default().trim();
                    blocks.push(HtmlBlock::Image {
                        src,
                        alt: SharedString::from(alt.to_string()),
                    });
                }
            }
        }
        "iframe" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            if let Some(video_block) = parse_video_block(&element_ref) {
                blocks.push(video_block);
            }
        }
        "video" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            if let Some(video_block) = parse_video_block(&element_ref) {
                blocks.push(video_block);
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            let mut spans = Vec::new();
            parse_inline_children(element_ref, &inline_style, &mut spans, image_urls);
            if !spans.is_empty() {
                let level = tag_name[1..].parse::<u8>().ok().unwrap_or(2);
                blocks.push(HtmlBlock::Heading { level, spans });
            }
        }
        "ul" | "ol" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            let ordered = tag_name == "ol";
            let mut items = Vec::new();
            for child in element_ref.children() {
                let Some(item_ref) = ElementRef::wrap(child) else {
                    continue;
                };
                if item_ref.value().name() != "li" {
                    continue;
                }
                let mut spans = Vec::new();
                parse_inline_children(
                    item_ref,
                    &HtmlInlineStyle::default(),
                    &mut spans,
                    image_urls,
                );
                if !spans.is_empty() {
                    items.push(spans);
                }
            }
            if !items.is_empty() {
                blocks.push(HtmlBlock::List { ordered, items });
            }
        }
        "blockquote" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            let mut spans = Vec::new();
            parse_inline_children(element_ref, &inline_style, &mut spans, image_urls);
            if !spans.is_empty() {
                blocks.push(HtmlBlock::Quote { spans });
            }
        }
        "p" => {
            flush_pending_paragraph(blocks, pending_paragraph);
            let mut spans = Vec::new();
            parse_inline_children(element_ref, &inline_style, &mut spans, image_urls);
            if !spans.is_empty() {
                blocks.push(HtmlBlock::Paragraph { spans });
            }
        }
        "div" | "section" | "article" | "main" => {
            if has_block_children(element_ref) {
                flush_pending_paragraph(blocks, pending_paragraph);
                for child in element_ref.children() {
                    match child.value() {
                        Node::Text(text) => {
                            push_inline_text(pending_paragraph, &text.text, inline_style.clone());
                        }
                        Node::Element(_) => {
                            if let Some(child_ref) = ElementRef::wrap(child) {
                                parse_element(
                                    child_ref,
                                    &inline_style,
                                    blocks,
                                    pending_paragraph,
                                    image_urls,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                let mut spans = Vec::new();
                parse_inline_children(element_ref, &inline_style, &mut spans, image_urls);
                if !spans.is_empty() {
                    blocks.push(HtmlBlock::Paragraph { spans });
                }
            }
        }
        _ => {
            parse_inline_children_into_pending(
                element_ref,
                &inline_style,
                pending_paragraph,
                image_urls,
            );
        }
    }
}

fn parse_inline_children(
    element_ref: ElementRef<'_>,
    inherited_style: &HtmlInlineStyle,
    spans: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    for child in element_ref.children() {
        match child.value() {
            Node::Text(text) => push_inline_text(spans, &text.text, inherited_style.clone()),
            Node::Element(_) => {
                if let Some(child_ref) = ElementRef::wrap(child) {
                    let tag_name = child_ref.value().name();
                    if tag_name == "img" {
                        if let Some(src) = child_ref.value().attr("src") {
                            let src = SharedString::from(src.trim().to_string());
                            if !src.trim().is_empty() {
                                image_urls.push(src);
                            }
                        }
                        continue;
                    }
                    if tag_name == "br" {
                        push_inline_text(spans, "\n", inherited_style.clone());
                        continue;
                    }
                    let next_style = merge_inline_style(tag_name, inherited_style, &child_ref);
                    parse_inline_children(child_ref, &next_style, spans, image_urls);
                    if matches!(tag_name, "p" | "div" | "li") {
                        push_inline_text(spans, "\n", HtmlInlineStyle::default());
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_inline_children_into_pending(
    element_ref: ElementRef<'_>,
    inherited_style: &HtmlInlineStyle,
    pending_paragraph: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    let mut spans = Vec::new();
    parse_inline_children(element_ref, inherited_style, &mut spans, image_urls);
    for span in spans {
        push_inline_text(pending_paragraph, &span.text, span.style);
    }
}

fn has_block_children(element_ref: ElementRef<'_>) -> bool {
    element_ref.children().any(|child| {
        let Some(child_ref) = ElementRef::wrap(child) else {
            return false;
        };
        matches!(
            child_ref.value().name(),
            "div"
                | "section"
                | "article"
                | "p"
                | "ul"
                | "ol"
                | "blockquote"
                | "img"
                | "iframe"
                | "video"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "hr"
        )
    })
}

fn parse_video_block(element_ref: &ElementRef<'_>) -> Option<HtmlBlock> {
    let src = if element_ref.value().name() == "video" {
        element_ref
            .value()
            .attr("src")
            .map(ToString::to_string)
            .or_else(|| {
                element_ref.children().find_map(|child| {
                    let child_ref = ElementRef::wrap(child)?;
                    if child_ref.value().name() != "source" {
                        return None;
                    }
                    child_ref.value().attr("src").map(ToString::to_string)
                })
            })
    } else {
        element_ref.value().attr("src").map(ToString::to_string)
    }?;

    let src = src.trim().to_string();
    if src.is_empty() {
        return None;
    }

    let provider = detect_video_provider(&src);
    let title = element_ref
        .value()
        .attr("title")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{provider} 视频"));

    Some(HtmlBlock::Video {
        src: SharedString::from(src),
        title: SharedString::from(title),
        provider: SharedString::from(provider),
    })
}

fn detect_video_provider(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.contains("youtube.com") || lower.contains("youtu.be") {
        "YouTube"
    } else if lower.contains("bilibili.com") || lower.contains("bilivideo.com") {
        "Bilibili"
    } else if lower.contains("vimeo.com") {
        "Vimeo"
    } else if lower.ends_with(".mp4")
        || lower.ends_with(".webm")
        || lower.ends_with(".mov")
        || lower.ends_with(".m3u8")
    {
        "直链视频"
    } else {
        "网络视频"
    }
}

fn flush_pending_paragraph(blocks: &mut Vec<HtmlBlock>, pending_paragraph: &mut Vec<HtmlInline>) {
    trim_trailing_breaks(pending_paragraph);
    if !pending_paragraph.is_empty() {
        blocks.push(HtmlBlock::Paragraph {
            spans: std::mem::take(pending_paragraph),
        });
    }
}

fn trim_trailing_breaks(spans: &mut Vec<HtmlInline>) {
    while spans.last().is_some_and(|span| span.text.trim().is_empty()) {
        spans.pop();
    }
}

fn push_inline_text(spans: &mut Vec<HtmlInline>, text: &str, style: HtmlInlineStyle) {
    let text = text.replace('\u{a0}', " ");
    if text.is_empty() {
        return;
    }

    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.text.push_str(&text);
        return;
    }

    spans.push(HtmlInline { text, style });
}

fn merge_inline_style(
    tag_name: &str,
    inherited_style: &HtmlInlineStyle,
    element_ref: &ElementRef<'_>,
) -> HtmlInlineStyle {
    let mut style = inherited_style.clone();

    match tag_name {
        "strong" | "b" => style.bold = true,
        "em" | "i" => style.italic = true,
        "u" => style.underline = true,
        "s" | "strike" | "del" => style.strike = true,
        "code" | "pre" => style.code = true,
        "a" => {
            style.underline = true;
            style.link = element_ref.value().attr("href").map(ToString::to_string);
        }
        _ => {}
    }

    if let Some(style_attr) = element_ref.value().attr("style") {
        for declaration in style_attr.split(';') {
            let mut parts = declaration.splitn(2, ':');
            let Some(name) = parts.next() else {
                continue;
            };
            let Some(value) = parts.next() else {
                continue;
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "color" {
                style.color = parse_css_color(value);
            }
        }
    }

    style
}

fn parse_css_color(value: &str) -> Option<Hsla> {
    let parsed = csscolorparser::parse(value).ok()?;
    Some(Hsla::from(Rgba {
        r: parsed.r as f32,
        g: parsed.g as f32,
        b: parsed.b as f32,
        a: parsed.a as f32,
    }))
}

fn collect_plain_text_lines(blocks: &[HtmlBlock]) -> Vec<SharedString> {
    let mut lines = Vec::new();
    for block in blocks {
        let text = match block {
            HtmlBlock::Heading { spans, .. }
            | HtmlBlock::Paragraph { spans }
            | HtmlBlock::Quote { spans } => spans_to_plain_text(spans),
            HtmlBlock::List { items, .. } => items
                .iter()
                .map(|item| spans_to_plain_text(item))
                .collect::<Vec<_>>()
                .join("\n"),
            HtmlBlock::Image { alt, .. } => alt.to_string(),
            HtmlBlock::Video {
                title, provider, ..
            } => format!("{provider} {title}"),
            HtmlBlock::Rule => String::new(),
        };
        for line in text.lines() {
            let normalized = line.trim();
            if normalized.chars().count() >= 16 {
                lines.push(SharedString::from(normalized.to_string()));
            }
        }
    }
    lines.truncate(8);
    lines
}

fn spans_to_plain_text(spans: &[HtmlInline]) -> String {
    spans
        .iter()
        .map(|span| span.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_block(
    block: &HtmlBlock,
    colors: &ThemeColors,
    image_cache: Option<&HashMap<SharedString, Arc<RenderImage>>>,
) -> AnyElement {
    match block {
        HtmlBlock::Heading { level, spans } => {
            let (font_size, line_height) = match *level {
                1 => (px(22.), px(32.)),
                2 => (px(20.), px(30.)),
                _ => (px(17.), px(26.)),
            };
            div()
                .w_full()
                .child(render_inline_text(
                    spans,
                    colors,
                    font_size,
                    line_height,
                    true,
                ))
                .into_any_element()
        }
        HtmlBlock::Paragraph { spans } => div()
            .w_full()
            .child(render_inline_text(spans, colors, px(14.), px(24.), false))
            .into_any_element(),
        HtmlBlock::List { ordered, items } => {
            let mut list = div().flex().flex_col().gap(px(8.));
            for (index, item) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}.", index + 1)
                } else {
                    "•".to_string()
                };
                list = list.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(18.))
                                .text_size(px(14.))
                                .line_height(px(24.))
                                .text_color(colors.text_secondary)
                                .child(marker),
                        )
                        .child(div().flex_1().min_w(px(0.)).child(render_inline_text(
                            item,
                            colors,
                            px(14.),
                            px(24.),
                            false,
                        ))),
                );
            }
            list.into_any_element()
        }
        HtmlBlock::Quote { spans } => div()
            .w_full()
            .pl(px(12.))
            .border_l_2()
            .border_color(colors.accent)
            .child(render_inline_text(spans, colors, px(14.), px(24.), false))
            .into_any_element(),
        HtmlBlock::Image { src, alt } => {
            let image_source = image_cache
                .and_then(|cache| cache.get(src))
                .cloned()
                .map(ImageSource::from)
                .unwrap_or_else(|| ImageSource::from(src.clone()));
            let block = div()
                .w_full()
                .rounded(px(18.))
                .overflow_hidden()
                .border_1()
                .border_color(Hsla {
                    a: 0.08,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.35,
                    ..colors.surface
                })
                .child(
                    img(image_source)
                        .w_full()
                        .max_w_full()
                        .max_h(px(420.))
                        .object_fit(ObjectFit::Cover),
                );

            if alt.trim().is_empty() {
                block.into_any_element()
            } else {
                block
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(10.))
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(alt.clone()),
                    )
                    .into_any_element()
            }
        }
        HtmlBlock::Video {
            src,
            title,
            provider,
        } => div()
            .w_full()
            .rounded(px(18.))
            .overflow_hidden()
            .cursor_pointer()
            .border_1()
            .border_color(Hsla {
                a: 0.08,
                ..colors.border
            })
            .bg(linear_gradient(
                135.0,
                linear_color_stop(
                    Hsla {
                        a: 0.20,
                        ..colors.accent
                    },
                    0.0,
                ),
                linear_color_stop(
                    Hsla {
                        a: 0.45,
                        ..colors.surface
                    },
                    1.0,
                ),
            ))
            .child(
                div()
                    .w_full()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(
                        div()
                            .w_full()
                            .h(px(180.))
                            .rounded(px(16.))
                            .bg(Hsla {
                                a: 0.36,
                                ..colors.surface
                            })
                            .border_1()
                            .border_color(Hsla {
                                a: 0.08,
                                ..colors.border
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(56.))
                                    .rounded(px(999.))
                                    .bg(colors.accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(themed_icon(
                                        lucide_gpui::icons::icon_play(),
                                        22.0,
                                        colors.btn_primary_text,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.text_muted)
                                            .child(provider.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.text_primary)
                                            .child(title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.text_secondary)
                                            .child("点击后将在浏览器中播放"),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(40.))
                                    .px(px(14.))
                                    .rounded(px(12.))
                                    .bg(colors.accent)
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.btn_primary_text)
                                    .child(themed_icon(
                                        lucide_gpui::icons::icon_external_link(),
                                        16.0,
                                        colors.btn_primary_text,
                                    ))
                                    .child("打开"),
                            ),
                    ),
            )
            .on_mouse_down(MouseButton::Left, {
                let src = src.clone();
                move |_ev, _window, cx| {
                    cx.open_url(src.as_ref());
                }
            })
            .into_any_element(),
        HtmlBlock::Rule => div()
            .h(px(1.))
            .w_full()
            .bg(colors.border)
            .into_any_element(),
    }
}

fn render_inline_text(
    spans: &[HtmlInline],
    colors: &ThemeColors,
    font_size: Pixels,
    line_height: Pixels,
    is_heading: bool,
) -> AnyElement {
    if spans.is_empty() {
        return div().into_any_element();
    }

    let mut combined_text = String::new();
    let mut runs = Vec::new();

    for span in spans {
        let text = span.text.replace("\r\n", "\n");
        if text.is_empty() {
            continue;
        }

        let color = span.style.color.unwrap_or_else(|| {
            if span.style.link.is_some() {
                colors.accent
            } else if is_heading || span.style.bold {
                colors.text_primary
            } else {
                colors.text_secondary
            }
        });

        runs.push(TextRun {
            len: text.len(),
            font: Font {
                family: if span.style.code {
                    "Cascadia Mono".into()
                } else {
                    "HarmonyOS Sans".into()
                },
                features: FontFeatures::default(),
                fallbacks: None,
                weight: if is_heading || span.style.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if span.style.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
            },
            color,
            background_color: span.style.code.then_some(Hsla {
                a: 0.18,
                ..colors.accent
            }),
            background_corner_radius: None,
            background_padding: None,
            underline: (span.style.underline || span.style.link.is_some()).then_some(
                UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                },
            ),
            strikethrough: span.style.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        });
        combined_text.push_str(&text);
    }

    div()
        .w_full()
        .text_size(font_size)
        .line_height(line_height)
        .whitespace_normal()
        .child(StyledText::new(SharedString::from(combined_text)).with_runs(runs))
        .into_any_element()
}
