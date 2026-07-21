use crate::ui::components::icon::themed_icon;
use crate::ui::theme::ThemeColors;
use ego_tree::NodeRef;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use scraper::{ElementRef, Html, Node, Selector};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const DEFAULT_ROOT_FONT_SIZE: f32 = 16.0;
const DEFAULT_PARAGRAPH_GAP: f32 = 14.0;
const DEFAULT_LINE_HEIGHT_RATIO: f32 = 1.6;
const DEFAULT_MAX_CSS_RULES: usize = 16_384;

/// A stylesheet already loaded by the caller.
///
/// The renderer intentionally does not perform network I/O. Use
/// [`discover_html_stylesheets`] to collect requests, load them through the
/// application's HTTP/cache layer, then pass the results through
/// [`HtmlRenderOptions::external_stylesheets`].
#[derive(Debug, Clone, Default)]
pub struct HtmlStyleSheet {
    pub href: Option<SharedString>,
    pub css: SharedString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlStylesheetRequest {
    pub href: SharedString,
    pub media: Option<SharedString>,
}

#[derive(Debug, Clone)]
pub struct HtmlRenderOptions {
    pub base_url: Option<SharedString>,
    pub external_stylesheets: Vec<HtmlStyleSheet>,
    pub user_css: Vec<SharedString>,
    pub root_font_size: f32,
    pub max_css_rules: usize,
}

impl Default for HtmlRenderOptions {
    fn default() -> Self {
        Self {
            base_url: None,
            external_stylesheets: Vec::new(),
            user_css: Vec::new(),
            root_font_size: DEFAULT_ROOT_FONT_SIZE,
            max_css_rules: DEFAULT_MAX_CSS_RULES,
        }
    }
}

impl HtmlRenderOptions {
    /// Returns the configured base URL as a plain string slice.
    ///
    /// `SharedString` dereferences to GPUI's internal `ArcCow<str>`, so
    /// `Option<SharedString>::as_deref()` does not produce `Option<&str>`.
    /// Keep the conversion in one place to avoid leaking that implementation
    /// detail through the renderer.
    #[inline]
    pub fn base_url_str(&self) -> Option<&str> {
        self.base_url.as_ref().map(SharedString::as_str)
    }
}

/// Synchronous adapter for applications that already have a local stylesheet
/// cache. Async applications should discover first, fetch asynchronously, and
/// call [`parse_html_document_with_options`] with the loaded sheets.
pub trait HtmlStylesheetLoader {
    fn load_stylesheet(&self, request: &HtmlStylesheetRequest) -> Result<Option<String>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HtmlCssLength {
    Auto,
    Px(f32),
    Percent(f32),
}

impl Default for HtmlCssLength {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HtmlCssEdges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HtmlTextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HtmlObjectFit {
    Cover,
    Fill,
    #[default]
    Contain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlBlockStyle {
    pub width: HtmlCssLength,
    pub height: HtmlCssLength,
    pub min_width: HtmlCssLength,
    pub min_height: HtmlCssLength,
    pub max_width: HtmlCssLength,
    pub max_height: HtmlCssLength,
    pub margin: HtmlCssEdges,
    pub padding: HtmlCssEdges,
    pub background_color: Option<Hsla>,
    pub border_color: Option<Hsla>,
    pub border_width: f32,
    pub border_radius: f32,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    pub text_align: HtmlTextAlign,
    pub opacity: f32,
    pub overflow_hidden: bool,
    pub object_fit: HtmlObjectFit,
    pub list_style_type: Option<SharedString>,
}

impl Default for HtmlBlockStyle {
    fn default() -> Self {
        Self {
            width: HtmlCssLength::Auto,
            height: HtmlCssLength::Auto,
            min_width: HtmlCssLength::Auto,
            min_height: HtmlCssLength::Auto,
            max_width: HtmlCssLength::Auto,
            max_height: HtmlCssLength::Auto,
            margin: HtmlCssEdges::default(),
            padding: HtmlCssEdges::default(),
            background_color: None,
            border_color: None,
            border_width: 0.0,
            border_radius: 0.0,
            font_size: None,
            line_height: None,
            text_align: HtmlTextAlign::Left,
            opacity: 1.0,
            overflow_hidden: false,
            object_fit: HtmlObjectFit::Contain,
            list_style_type: None,
        }
    }
}

impl HtmlBlockStyle {
    fn is_visually_empty(&self) -> bool {
        self.width == HtmlCssLength::Auto
            && self.height == HtmlCssLength::Auto
            && self.min_width == HtmlCssLength::Auto
            && self.min_height == HtmlCssLength::Auto
            && self.max_width == HtmlCssLength::Auto
            && self.max_height == HtmlCssLength::Auto
            && self.margin == HtmlCssEdges::default()
            && self.padding == HtmlCssEdges::default()
            && self.background_color.is_none()
            && self.border_color.is_none()
            && self.border_width <= 0.0
            && self.border_radius <= 0.0
            && self.opacity >= 0.999
            && !self.overflow_hidden
    }
}

#[derive(Debug, Clone, Default)]
pub struct HtmlDocument {
    pub blocks: Vec<HtmlBlock>,
    /// Parallel to `blocks`, allowing CSS box data to remain separate from the
    /// semantic block payload.
    pub block_styles: Vec<HtmlBlockStyle>,
    pub image_urls: Vec<SharedString>,
    pub plain_text_lines: Vec<SharedString>,
    pub stylesheet_requests: Vec<HtmlStylesheetRequest>,
    pub css_diagnostics: Vec<SharedString>,
}

/// A single list item, optionally containing a nested sub-list.
#[derive(Debug, Clone)]
pub struct HtmlListItem {
    pub spans: Vec<HtmlInline>,
    pub sub_list: Option<(bool, Vec<HtmlListItem>)>,
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
        items: Vec<HtmlListItem>,
    },
    Quote {
        spans: Vec<HtmlInline>,
    },
    CodeBlock {
        code: SharedString,
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
    Table {
        headers: Vec<Vec<HtmlInline>>,
        rows: Vec<Vec<Vec<HtmlInline>>>,
    },
    /// Preserves CSS boxes for structural elements such as `div`, `section`,
    /// `article`, `figure`, and `details`.
    Group {
        blocks: Vec<HtmlBlock>,
        styles: Vec<HtmlBlockStyle>,
    },
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlInline {
    pub text: String,
    pub style: HtmlInlineStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlInlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
    pub color: Option<Hsla>,
    pub background_color: Option<Hsla>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    pub font_family: Option<String>,
    pub opacity: f32,
    pub preserve_whitespace: bool,
}

impl Default for HtmlInlineStyle {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            code: false,
            link: None,
            color: None,
            background_color: None,
            font_size: None,
            line_height: None,
            font_family: None,
            opacity: 1.0,
            preserve_whitespace: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HtmlDisplay {
    None,
    Inline,
    #[default]
    Block,
}

#[derive(Debug, Clone)]
struct CssComputedStyle {
    display: HtmlDisplay,
    inline: HtmlInlineStyle,
    block: HtmlBlockStyle,
    custom_properties: HashMap<String, String>,
}

impl Default for CssComputedStyle {
    fn default() -> Self {
        Self {
            display: HtmlDisplay::Block,
            inline: HtmlInlineStyle::default(),
            block: HtmlBlockStyle::default(),
            custom_properties: HashMap::new(),
        }
    }
}

impl CssComputedStyle {
    fn inherited(parent: &Self) -> Self {
        let mut style = Self::default();
        style.inline.color = parent.inline.color;
        style.inline.font_size = parent.inline.font_size;
        style.inline.line_height = parent.inline.line_height;
        style.inline.font_family = parent.inline.font_family.clone();
        style.inline.bold = parent.inline.bold;
        style.inline.italic = parent.inline.italic;
        style.inline.preserve_whitespace = parent.inline.preserve_whitespace;
        style.block.font_size = parent.block.font_size;
        style.block.line_height = parent.block.line_height;
        style.block.text_align = parent.block.text_align;
        style.custom_properties = parent.custom_properties.clone();
        style
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CssCombinator {
    Descendant,
    Child,
}

#[derive(Debug, Clone, Default)]
struct CssCompoundSelector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attributes: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
struct CssSelector {
    compounds: Vec<CssCompoundSelector>,
    combinators: Vec<CssCombinator>,
    specificity: u32,
}

#[derive(Debug, Clone)]
struct CssDeclaration {
    name: String,
    value: String,
    important: bool,
}

#[derive(Debug, Clone)]
struct CssRule {
    selector: CssSelector,
    declarations: Vec<CssDeclaration>,
    order: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CascadeRank {
    important: bool,
    specificity: u32,
    order: u32,
}

/// Compiled selector/declaration data. Rules are indexed by the right-most id,
/// class, and tag so rendering does not scan the complete stylesheet for every
/// DOM element.
#[derive(Debug, Clone, Default)]
pub struct CompiledHtmlCss {
    rules: Vec<CssRule>,
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    universal: Vec<usize>,
    diagnostics: Vec<SharedString>,
}

impl CompiledHtmlCss {
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn diagnostics(&self) -> &[SharedString] {
        &self.diagnostics
    }

    pub fn compile(html: &str, options: &HtmlRenderOptions) -> Self {
        let fragment = Html::parse_fragment(html);
        let mut css_sources = Vec::<String>::new();

        if let Ok(selector) = Selector::parse("style") {
            for style in fragment.select(&selector) {
                let css = style.text().collect::<String>();
                if !css.trim().is_empty() {
                    css_sources.push(css);
                }
            }
        }

        for sheet in &options.external_stylesheets {
            if !sheet.css.trim().is_empty() {
                css_sources.push(sheet.css.to_string());
            }
        }
        css_sources.extend(options.user_css.iter().map(ToString::to_string));

        let mut compiled = Self::default();
        let max_rules = options.max_css_rules.max(1);
        for source in css_sources {
            compile_css_source(&source, max_rules, &mut compiled);
            if compiled.rules.len() >= max_rules {
                compiled.diagnostics.push(SharedString::from(format!(
                    "CSS rule limit reached ({max_rules}); remaining rules were ignored"
                )));
                break;
            }
        }
        compiled.rebuild_indexes();
        compiled
    }

    fn rebuild_indexes(&mut self) {
        self.by_id.clear();
        self.by_class.clear();
        self.by_tag.clear();
        self.universal.clear();

        for (index, rule) in self.rules.iter().enumerate() {
            let Some(rightmost) = rule.selector.compounds.last() else {
                self.universal.push(index);
                continue;
            };
            let mut indexed = false;
            if let Some(id) = &rightmost.id {
                self.by_id.entry(id.clone()).or_default().push(index);
                indexed = true;
            }
            for class in &rightmost.classes {
                self.by_class
                    .entry(class.clone())
                    .or_default()
                    .push(index);
                indexed = true;
            }
            if let Some(tag) = &rightmost.tag {
                self.by_tag.entry(tag.clone()).or_default().push(index);
                indexed = true;
            }
            if !indexed {
                self.universal.push(index);
            }
        }
    }

    fn candidate_rule_indexes(&self, element: &ElementRef<'_>) -> Vec<usize> {
        let mut candidates = HashSet::new();
        candidates.extend(self.universal.iter().copied());

        if let Some(id) = element.value().attr("id") {
            if let Some(indexes) = self.by_id.get(&id.to_ascii_lowercase()) {
                candidates.extend(indexes.iter().copied());
            }
        }
        if let Some(classes) = element.value().attr("class") {
            for class in classes.split_ascii_whitespace() {
                if let Some(indexes) = self.by_class.get(&class.to_ascii_lowercase()) {
                    candidates.extend(indexes.iter().copied());
                }
            }
        }
        let tag = element.value().name().to_ascii_lowercase();
        if let Some(indexes) = self.by_tag.get(&tag) {
            candidates.extend(indexes.iter().copied());
        }

        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        candidates.sort_unstable();
        candidates
    }

    fn compute_style(
        &self,
        element: &ElementRef<'_>,
        parent: &CssComputedStyle,
        options: &HtmlRenderOptions,
    ) -> CssComputedStyle {
        let tag = element.value().name().to_ascii_lowercase();
        let mut style = CssComputedStyle::inherited(parent);
        apply_user_agent_style(&tag, &mut style);
        apply_semantic_inline_style(&tag, element, &mut style.inline);

        let mut winners: HashMap<String, (CascadeRank, String)> = HashMap::new();
        for index in self.candidate_rule_indexes(element) {
            let Some(rule) = self.rules.get(index) else {
                continue;
            };
            if !selector_matches(&rule.selector, *element) {
                continue;
            }
            for declaration in &rule.declarations {
                let rank = CascadeRank {
                    important: declaration.important,
                    specificity: rule.selector.specificity,
                    order: rule.order,
                };
                update_cascade_winner(
                    &mut winners,
                    declaration.name.clone(),
                    declaration.value.clone(),
                    rank,
                );
            }
        }

        if let Some(inline_css) = element.value().attr("style") {
            for declaration in parse_css_declarations(inline_css) {
                update_cascade_winner(
                    &mut winners,
                    declaration.name,
                    declaration.value,
                    CascadeRank {
                        important: declaration.important,
                        specificity: 1_000_000,
                        order: u32::MAX,
                    },
                );
            }
        }

        // Custom properties are inherited and must be installed before var() is resolved.
        let mut custom_names = winners
            .keys()
            .filter(|name| name.starts_with("--"))
            .cloned()
            .collect::<Vec<_>>();
        custom_names.sort();
        for name in custom_names {
            if let Some((_, value)) = winners.get(&name) {
                style.custom_properties.insert(name, value.clone());
            }
        }

        // Font-size affects em-based values, so apply it before the remaining declarations.
        if let Some((_, value)) = winners.get("font-size") {
            let value = resolve_css_vars(value, &style.custom_properties, 8);
            apply_css_property("font-size", &value, &mut style, options);
        }

        let mut declarations = winners.into_iter().collect::<Vec<_>>();
        declarations.sort_by(|a, b| a.1 .0.cmp(&b.1 .0));
        for (name, (_, value)) in declarations {
            if name.starts_with("--") || name == "font-size" {
                continue;
            }
            let value = resolve_css_vars(&value, &style.custom_properties, 8);
            apply_css_property(&name, &value, &mut style, options);
        }

        if style.display == HtmlDisplay::Inline {
            style.inline.background_color = style.block.background_color.take();
        }
        if tag == "a" {
            if let Some(link) = style.inline.link.take() {
                style.inline.link = Some(resolve_resource_url(options.base_url_str(), &link));
            }
        }
        style
    }
}

fn update_cascade_winner(
    winners: &mut HashMap<String, (CascadeRank, String)>,
    name: String,
    value: String,
    rank: CascadeRank,
) {
    let replace = winners
        .get(&name)
        .map(|(current, _)| rank >= *current)
        .unwrap_or(true);
    if replace {
        winners.insert(name, (rank, value));
    }
}

pub fn discover_html_stylesheets(
    html: &str,
    base_url: Option<&str>,
) -> Vec<HtmlStylesheetRequest> {
    let fragment = Html::parse_fragment(html);
    let document_base = Selector::parse("base[href]")
        .ok()
        .and_then(|selector| fragment.select(&selector).next())
        .and_then(|element| element.value().attr("href"))
        .map(str::to_string);
    let resolved_document_base = document_base
        .as_deref()
        .map(|value| resolve_resource_url(base_url, value));
    let effective_base = resolved_document_base.as_deref().or(base_url);

    let Ok(selector) = Selector::parse("link[href]") else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut requests = Vec::new();
    for link in fragment.select(&selector) {
        let rel = link.value().attr("rel").unwrap_or_default();
        if !rel
            .split_ascii_whitespace()
            .any(|part| part.eq_ignore_ascii_case("stylesheet"))
        {
            continue;
        }
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let media = link.value().attr("media").map(str::trim).filter(|value| !value.is_empty());
        if media.is_some_and(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("print") && !lower.contains("screen") && !lower.contains("all")
        }) {
            continue;
        }
        let href = resolve_resource_url(effective_base, href);
        if href.is_empty() || !seen.insert(href.clone()) {
            continue;
        }
        requests.push(HtmlStylesheetRequest {
            href: SharedString::from(href),
            media: media.map(|value| SharedString::from(value.to_string())),
        });
    }
    requests
}

pub fn parse_html_document(html: &str) -> HtmlDocument {
    parse_html_document_with_options(html, &HtmlRenderOptions::default())
}

pub fn parse_html_document_with_loader<L: HtmlStylesheetLoader>(
    html: &str,
    options: &HtmlRenderOptions,
    loader: &L,
) -> HtmlDocument {
    let requests = discover_html_stylesheets(html, options.base_url_str());
    let mut resolved = options.clone();
    let mut load_diagnostics = Vec::new();
    for request in &requests {
        match loader.load_stylesheet(request) {
            Ok(Some(css)) => resolved.external_stylesheets.push(HtmlStyleSheet {
                href: Some(request.href.clone()),
                css: SharedString::from(css),
            }),
            Ok(None) => {}
            Err(error) => load_diagnostics.push(SharedString::from(format!(
                "failed to load stylesheet {}: {error}",
                request.href
            ))),
        }
    }
    let mut document = parse_html_document_with_options(html, &resolved);
    document.css_diagnostics.extend(load_diagnostics);
    document
}

pub fn parse_html_document_with_options(
    html: &str,
    options: &HtmlRenderOptions,
) -> HtmlDocument {
    let html = html.trim();
    if html.is_empty() {
        return HtmlDocument::default();
    }

    let fragment = Html::parse_fragment(html);
    let css = CompiledHtmlCss::compile(html, options);
    parse_html_document_with_compiled_css(html, &fragment, &css, options)
}

pub fn parse_html_document_with_precompiled_css(
    html: &str,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
) -> HtmlDocument {
    let html = html.trim();
    if html.is_empty() {
        return HtmlDocument::default();
    }
    let fragment = Html::parse_fragment(html);
    parse_html_document_with_compiled_css(html, &fragment, css, options)
}

fn parse_html_document_with_compiled_css(
    html: &str,
    fragment: &Html,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
) -> HtmlDocument {
    let root = fragment.tree.root();
    let mut blocks = Vec::new();
    let mut block_styles = Vec::new();
    let mut pending_paragraph = Vec::new();
    let mut image_urls = Vec::new();
    let root_style = CssComputedStyle::default();

    parse_nodes(
        root,
        &root_style,
        css,
        options,
        &mut blocks,
        &mut block_styles,
        &mut pending_paragraph,
        &mut image_urls,
    );
    flush_pending_paragraph(
        &mut blocks,
        &mut block_styles,
        &mut pending_paragraph,
    );

    let plain_text_lines = collect_plain_text_lines(&blocks);
    let stylesheet_requests = discover_html_stylesheets(html, options.base_url_str());

    HtmlDocument {
        blocks,
        block_styles,
        image_urls,
        plain_text_lines,
        stylesheet_requests,
        css_diagnostics: css.diagnostics.clone(),
    }
}

pub fn render_html_document(
    document: &HtmlDocument,
    colors: &ThemeColors,
    image_cache: Option<&HashMap<SharedString, Arc<RenderImage>>>,
) -> Div {
    let mut column = div().w_full().flex().flex_col();

    for (index, block) in document.blocks.iter().enumerate() {
        let style = document.block_styles.get(index).cloned().unwrap_or_default();
        column = column.child(render_block(block, &style, colors, image_cache));
    }

    column
}

fn parse_nodes(
    parent: NodeRef<'_, Node>,
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    blocks: &mut Vec<HtmlBlock>,
    block_styles: &mut Vec<HtmlBlockStyle>,
    pending_paragraph: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    for child in parent.children() {
        match child.value() {
            Node::Text(text) => {
                push_inline_text(
                    pending_paragraph,
                    &text.text,
                    inherited_style.inline.clone(),
                );
            }
            Node::Element(_) => {
                if let Some(element_ref) = ElementRef::wrap(child) {
                    parse_element(
                        element_ref,
                        inherited_style,
                        css,
                        options,
                        blocks,
                        block_styles,
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
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    blocks: &mut Vec<HtmlBlock>,
    block_styles: &mut Vec<HtmlBlockStyle>,
    pending_paragraph: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    let tag_name = element_ref.value().name().to_ascii_lowercase();
    if matches!(
        tag_name.as_str(),
        "style" | "script" | "noscript" | "template" | "meta" | "link" | "base" | "head"
    ) {
        return;
    }

    let computed = css.compute_style(&element_ref, inherited_style, options);
    if computed.display == HtmlDisplay::None {
        return;
    }
    let inline_style = computed.inline.clone();
    let block_style = computed.block.clone();

    match tag_name.as_str() {
        "br" => push_inline_text(pending_paragraph, "\n", inline_style),
        "hr" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            push_block(blocks, block_styles, HtmlBlock::Rule, block_style);
        }
        "img" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            if let Some(src) = element_ref.value().attr("src") {
                let src = resolve_resource_url(options.base_url_str(), src);
                if !src.trim().is_empty() {
                    let src = SharedString::from(src);
                    image_urls.push(src.clone());
                    let alt = element_ref.value().attr("alt").unwrap_or_default().trim();
                    push_block(
                        blocks,
                        block_styles,
                        HtmlBlock::Image {
                            src,
                            alt: SharedString::from(alt.to_string()),
                        },
                        block_style,
                    );
                }
            }
        }
        "iframe" | "video" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            if let Some(video_block) = parse_video_block(&element_ref, options.base_url_str()) {
                push_block(blocks, block_styles, video_block, block_style);
            }
        }
        "pre" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            let code = extract_text_content(element_ref);
            let code = code.trim_matches('\n').to_string();
            if !code.is_empty() {
                push_block(
                    blocks,
                    block_styles,
                    HtmlBlock::CodeBlock {
                        code: SharedString::from(code),
                    },
                    block_style,
                );
            }
        }
        "table" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            if let Some(table_block) =
                parse_table_block(&element_ref, &computed, css, options, image_urls)
            {
                push_block(blocks, block_styles, table_block, block_style);
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            let mut spans = Vec::new();
            parse_inline_children(
                element_ref,
                &computed,
                css,
                options,
                &mut spans,
                image_urls,
            );
            trim_trailing_breaks(&mut spans);
            if !spans.is_empty() {
                let level = tag_name[1..].parse::<u8>().ok().unwrap_or(2);
                push_block(
                    blocks,
                    block_styles,
                    HtmlBlock::Heading { level, spans },
                    block_style,
                );
            }
        }
        "ul" | "ol" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            let ordered = tag_name == "ol";
            let items = parse_list_items(element_ref, &computed, css, options, image_urls);
            if !items.is_empty() {
                push_block(
                    blocks,
                    block_styles,
                    HtmlBlock::List { ordered, items },
                    block_style,
                );
            }
        }
        "blockquote" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            let mut spans = Vec::new();
            parse_inline_children(
                element_ref,
                &computed,
                css,
                options,
                &mut spans,
                image_urls,
            );
            trim_trailing_breaks(&mut spans);
            if !spans.is_empty() {
                push_block(
                    blocks,
                    block_styles,
                    HtmlBlock::Quote { spans },
                    block_style,
                );
            }
        }
        "p" | "address" | "figcaption" | "summary" | "dt" | "dd" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            let mut spans = Vec::new();
            parse_inline_children(
                element_ref,
                &computed,
                css,
                options,
                &mut spans,
                image_urls,
            );
            trim_trailing_breaks(&mut spans);
            if !spans.is_empty() {
                push_block(
                    blocks,
                    block_styles,
                    HtmlBlock::Paragraph { spans },
                    block_style,
                );
            }
        }
        "input" => {
            let text = element_ref
                .value()
                .attr("value")
                .or_else(|| element_ref.value().attr("placeholder"))
                .unwrap_or_default();
            push_inline_text(pending_paragraph, text, inline_style);
        }
        "div" | "section" | "article" | "main" | "figure" | "details" | "aside"
        | "header" | "footer" | "nav" | "dl" => {
            flush_pending_paragraph(blocks, block_styles, pending_paragraph);
            let mut child_blocks = Vec::new();
            let mut child_styles = Vec::new();
            let mut child_pending = Vec::new();
            parse_nodes(
                *element_ref,
                &computed,
                css,
                options,
                &mut child_blocks,
                &mut child_styles,
                &mut child_pending,
                image_urls,
            );
            flush_pending_paragraph(&mut child_blocks, &mut child_styles, &mut child_pending);
            if !child_blocks.is_empty() {
                if block_style.is_visually_empty() {
                    blocks.extend(child_blocks);
                    block_styles.extend(child_styles);
                } else {
                    push_block(
                        blocks,
                        block_styles,
                        HtmlBlock::Group {
                            blocks: child_blocks,
                            styles: child_styles,
                        },
                        block_style,
                    );
                }
            }
        }
        // Structural table elements are handled entirely inside parse_table_block.
        "thead" | "tbody" | "tfoot" | "tr" | "th" | "td" | "caption" | "colgroup"
        | "col" | "source" | "track" => {}
        _ => {
            parse_inline_children_into_pending(
                element_ref,
                &computed,
                css,
                options,
                pending_paragraph,
                image_urls,
            );
            if computed.display == HtmlDisplay::Block {
                push_inline_text(pending_paragraph, "\n", HtmlInlineStyle::default());
            }
        }
    }
}

fn push_block(
    blocks: &mut Vec<HtmlBlock>,
    styles: &mut Vec<HtmlBlockStyle>,
    block: HtmlBlock,
    style: HtmlBlockStyle,
) {
    blocks.push(block);
    styles.push(style);
}

fn parse_list_items(
    element_ref: ElementRef<'_>,
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    image_urls: &mut Vec<SharedString>,
) -> Vec<HtmlListItem> {
    let mut items = Vec::new();
    for child in element_ref.children() {
        let Some(item_ref) = ElementRef::wrap(child) else {
            continue;
        };
        if item_ref.value().name() != "li" {
            continue;
        }

        let item_style = css.compute_style(&item_ref, inherited_style, options);
        if item_style.display == HtmlDisplay::None {
            continue;
        }
        let mut spans = Vec::new();
        let mut sub_list = None;

        for li_child in item_ref.children() {
            match li_child.value() {
                Node::Text(text) => {
                    push_inline_text(&mut spans, &text.text, item_style.inline.clone());
                }
                Node::Element(_) => {
                    if let Some(child_ref) = ElementRef::wrap(li_child) {
                        let child_tag = child_ref.value().name();
                        if child_tag == "ul" || child_tag == "ol" {
                            let nested_style = css.compute_style(&child_ref, &item_style, options);
                            let nested =
                                parse_list_items(child_ref, &nested_style, css, options, image_urls);
                            if !nested.is_empty() {
                                sub_list = Some((child_tag == "ol", nested));
                            }
                        } else {
                            let child_style = css.compute_style(&child_ref, &item_style, options);
                            if child_style.display != HtmlDisplay::None {
                                parse_inline_children(
                                    child_ref,
                                    &child_style,
                                    css,
                                    options,
                                    &mut spans,
                                    image_urls,
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        trim_trailing_breaks(&mut spans);
        if !spans.is_empty() || sub_list.is_some() {
            items.push(HtmlListItem { spans, sub_list });
        }
    }
    items
}

fn parse_table_block(
    element_ref: &ElementRef<'_>,
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    image_urls: &mut Vec<SharedString>,
) -> Option<HtmlBlock> {
    let mut headers = Vec::new();
    let mut rows = Vec::new();

    for child in element_ref.children() {
        let Some(child_ref) = ElementRef::wrap(child) else {
            continue;
        };
        match child_ref.value().name() {
            "thead" => {
                let section_style = css.compute_style(&child_ref, inherited_style, options);
                for tr in child_ref.children().filter_map(ElementRef::wrap) {
                    if tr.value().name() == "tr" {
                        let cells = parse_table_row(
                            tr,
                            &section_style,
                            css,
                            options,
                            image_urls,
                        );
                        if !cells.is_empty() {
                            if headers.is_empty() {
                                headers = cells;
                            } else {
                                rows.push(cells);
                            }
                        }
                    }
                }
            }
            "tbody" | "tfoot" => {
                let section_style = css.compute_style(&child_ref, inherited_style, options);
                for tr in child_ref.children().filter_map(ElementRef::wrap) {
                    if tr.value().name() == "tr" {
                        let cells = parse_table_row(
                            tr,
                            &section_style,
                            css,
                            options,
                            image_urls,
                        );
                        if !cells.is_empty() {
                            rows.push(cells);
                        }
                    }
                }
            }
            "tr" => {
                let row_style = css.compute_style(&child_ref, inherited_style, options);
                let cells = parse_table_row(
                    child_ref,
                    &row_style,
                    css,
                    options,
                    image_urls,
                );
                if !cells.is_empty() {
                    let all_th = child_ref
                        .children()
                        .filter_map(ElementRef::wrap)
                        .all(|cell| cell.value().name() == "th");
                    if all_th && headers.is_empty() {
                        headers = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            _ => {}
        }
    }

    if headers.is_empty() && rows.is_empty() {
        None
    } else {
        Some(HtmlBlock::Table { headers, rows })
    }
}

fn parse_table_row(
    row_ref: ElementRef<'_>,
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    image_urls: &mut Vec<SharedString>,
) -> Vec<Vec<HtmlInline>> {
    let mut cells = Vec::new();
    for cell in row_ref.children().filter_map(ElementRef::wrap) {
        if !matches!(cell.value().name(), "th" | "td") {
            continue;
        }
        let cell_style = css.compute_style(&cell, inherited_style, options);
        if cell_style.display == HtmlDisplay::None {
            continue;
        }
        let mut spans = Vec::new();
        parse_inline_children(
            cell,
            &cell_style,
            css,
            options,
            &mut spans,
            image_urls,
        );
        trim_trailing_breaks(&mut spans);
        cells.push(spans);
    }
    cells
}

fn extract_text_content(element_ref: ElementRef<'_>) -> String {
    let mut text = String::new();
    collect_text_recursive(element_ref, &mut text);
    text
}

fn collect_text_recursive(element_ref: ElementRef<'_>, text: &mut String) {
    for child in element_ref.children() {
        match child.value() {
            Node::Text(value) => text.push_str(&value.text),
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    if child_el.value().name() == "br" {
                        text.push('\n');
                    } else {
                        collect_text_recursive(child_el, text);
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_inline_children(
    element_ref: ElementRef<'_>,
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    spans: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    for child in element_ref.children() {
        match child.value() {
            Node::Text(text) => {
                push_inline_text(spans, &text.text, inherited_style.inline.clone());
            }
            Node::Element(_) => {
                let Some(child_ref) = ElementRef::wrap(child) else {
                    continue;
                };
                let tag_name = child_ref.value().name().to_ascii_lowercase();
                if matches!(tag_name.as_str(), "style" | "script" | "template") {
                    continue;
                }
                let child_style = css.compute_style(&child_ref, inherited_style, options);
                if child_style.display == HtmlDisplay::None {
                    continue;
                }
                if tag_name == "img" {
                    if let Some(src) = child_ref.value().attr("src") {
                        let src = resolve_resource_url(options.base_url_str(), src);
                        if !src.is_empty() {
                            image_urls.push(SharedString::from(src));
                        }
                    }
                    let alt = child_ref.value().attr("alt").unwrap_or_default();
                    if !alt.is_empty() {
                        push_inline_text(spans, alt, child_style.inline.clone());
                    }
                    continue;
                }
                if tag_name == "br" {
                    push_inline_text(spans, "\n", child_style.inline.clone());
                    continue;
                }
                if tag_name == "input" {
                    let text = child_ref
                        .value()
                        .attr("value")
                        .or_else(|| child_ref.value().attr("placeholder"))
                        .unwrap_or_default();
                    push_inline_text(spans, text, child_style.inline.clone());
                    continue;
                }

                parse_inline_children(
                    child_ref,
                    &child_style,
                    css,
                    options,
                    spans,
                    image_urls,
                );
                if child_style.display == HtmlDisplay::Block
                    || matches!(tag_name.as_str(), "p" | "div" | "li" | "tr")
                {
                    push_inline_text(spans, "\n", HtmlInlineStyle::default());
                }
            }
            _ => {}
        }
    }
}

fn parse_inline_children_into_pending(
    element_ref: ElementRef<'_>,
    inherited_style: &CssComputedStyle,
    css: &CompiledHtmlCss,
    options: &HtmlRenderOptions,
    pending_paragraph: &mut Vec<HtmlInline>,
    image_urls: &mut Vec<SharedString>,
) {
    let mut spans = Vec::new();
    parse_inline_children(
        element_ref,
        inherited_style,
        css,
        options,
        &mut spans,
        image_urls,
    );
    for span in spans {
        push_inline_text(pending_paragraph, &span.text, span.style);
    }
}

fn parse_video_block(element_ref: &ElementRef<'_>, base_url: Option<&str>) -> Option<HtmlBlock> {
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

    let src = resolve_resource_url(base_url, src.trim());
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

fn flush_pending_paragraph(
    blocks: &mut Vec<HtmlBlock>,
    styles: &mut Vec<HtmlBlockStyle>,
    pending_paragraph: &mut Vec<HtmlInline>,
) {
    trim_trailing_breaks(pending_paragraph);
    if !pending_paragraph.is_empty() {
        let mut style = HtmlBlockStyle::default();
        style.margin.bottom = DEFAULT_PARAGRAPH_GAP;
        push_block(
            blocks,
            styles,
            HtmlBlock::Paragraph {
                spans: std::mem::take(pending_paragraph),
            },
            style,
        );
    }
}

fn trim_trailing_breaks(spans: &mut Vec<HtmlInline>) {
    while spans.last().is_some_and(|span| span.text.trim().is_empty()) {
        spans.pop();
    }
}

fn push_inline_text(spans: &mut Vec<HtmlInline>, text: &str, style: HtmlInlineStyle) {
    let mut text = text.replace('\u{a0}', " ");
    if !style.preserve_whitespace {
        text = collapse_html_whitespace(&text);
    }
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

fn collapse_html_whitespace(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_whitespace = false;
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
            if !in_whitespace {
                output.push(' ');
                in_whitespace = true;
            }
        } else {
            output.push(ch);
            in_whitespace = false;
        }
    }
    output
}

fn collect_plain_text_lines(blocks: &[HtmlBlock]) -> Vec<SharedString> {
    fn collect(blocks: &[HtmlBlock], lines: &mut Vec<SharedString>) {
        for block in blocks {
            let text = match block {
                HtmlBlock::Heading { spans, .. }
                | HtmlBlock::Paragraph { spans }
                | HtmlBlock::Quote { spans } => spans_to_plain_text(spans),
                HtmlBlock::List { items, .. } => items
                    .iter()
                    .map(|item| spans_to_plain_text(&item.spans))
                    .collect::<Vec<_>>()
                    .join("\n"),
                HtmlBlock::Image { alt, .. } => alt.to_string(),
                HtmlBlock::Video {
                    title, provider, ..
                } => format!("{provider} {title}"),
                HtmlBlock::CodeBlock { code } => code.to_string(),
                HtmlBlock::Table { headers, rows } => {
                    let header_text = headers
                        .iter()
                        .map(|cell| spans_to_plain_text(cell))
                        .collect::<Vec<_>>()
                        .join(" | ");
                    let row_texts = rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|cell| spans_to_plain_text(cell))
                                .collect::<Vec<_>>()
                                .join(" | ")
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("{header_text}\n{row_texts}")
                }
                HtmlBlock::Group { blocks, .. } => {
                    collect(blocks, lines);
                    String::new()
                }
                HtmlBlock::Rule => String::new(),
            };
            for line in text.lines() {
                let normalized = line.trim();
                if normalized.chars().count() >= 16 {
                    lines.push(SharedString::from(normalized.to_string()));
                }
            }
        }
    }

    let mut lines = Vec::new();
    collect(blocks, &mut lines);
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

// ------------------------------ CSS compiler ------------------------------

fn compile_css_source(source: &str, max_rules: usize, compiled: &mut CompiledHtmlCss) {
    let source = strip_css_comments(source);
    compile_css_rule_list(&source, max_rules, compiled);
}

fn compile_css_rule_list(source: &str, max_rules: usize, compiled: &mut CompiledHtmlCss) {
    let mut cursor = 0usize;
    while cursor < source.len() && compiled.rules.len() < max_rules {
        cursor = skip_css_whitespace(&source, cursor);
        if cursor >= source.len() {
            break;
        }

        if source[cursor..].starts_with("@media") || source[cursor..].starts_with("@supports") {
            let Some(open) = find_css_char(&source, cursor, '{') else {
                break;
            };
            let condition = source[cursor..open].to_ascii_lowercase();
            let Some(close) = find_matching_css_brace(&source, open) else {
                break;
            };
            let should_include = !condition.contains("print")
                && !condition.contains("prefers-reduced-motion: no-preference");
            if should_include {
                compile_css_rule_list(&source[open + 1..close], max_rules, compiled);
            }
            cursor = close + 1;
            continue;
        }

        if source[cursor..].starts_with('@') {
            if let Some(open) = find_css_char(&source, cursor, '{') {
                if let Some(close) = find_matching_css_brace(&source, open) {
                    cursor = close + 1;
                    continue;
                }
            }
            cursor = source[cursor..]
                .find(';')
                .map(|offset| cursor + offset + 1)
                .unwrap_or(source.len());
            continue;
        }

        let Some(open) = find_css_char(&source, cursor, '{') else {
            break;
        };
        let Some(close) = find_matching_css_brace(&source, open) else {
            compiled
                .diagnostics
                .push(SharedString::from("unterminated CSS rule"));
            break;
        };
        let selector_text = source[cursor..open].trim();
        let declarations = parse_css_declarations(&source[open + 1..close]);
        if !selector_text.is_empty() && !declarations.is_empty() {
            for selector_text in split_css_top_level(selector_text, ',') {
                if compiled.rules.len() >= max_rules {
                    break;
                }
                if let Some(selector) = parse_css_selector(selector_text.trim()) {
                    let order = compiled.rules.len() as u32;
                    compiled.rules.push(CssRule {
                        selector,
                        declarations: declarations.clone(),
                        order,
                    });
                }
            }
        }
        cursor = close + 1;
    }
}

fn strip_css_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            index += 2;
            while index + 1 < bytes.len()
                && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
            {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
        } else {
            output.push(bytes[index] as char);
            index += 1;
        }
    }
    output
}

fn skip_css_whitespace(source: &str, mut cursor: usize) -> usize {
    while cursor < source.len() && source.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn find_css_char(source: &str, start: usize, target: char) -> Option<usize> {
    let mut quote = None;
    let mut bracket_depth = 0u32;
    let mut paren_depth = 0u32;
    let mut escaped = false;
    for (offset, ch) in source[start..].char_indices() {
        let index = start + offset;
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        match ch {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            _ => {}
        }
        if ch == target && bracket_depth == 0 && paren_depth == 0 {
            return Some(index);
        }
    }
    None
}

fn find_matching_css_brace(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0u32;
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in source[open..].char_indices() {
        let index = open + offset;
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn split_css_top_level(source: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut quote = None;
    let mut bracket_depth = 0u32;
    let mut paren_depth = 0u32;
    let mut escaped = false;
    for (index, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        match ch {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            _ => {}
        }
        if ch == separator && bracket_depth == 0 && paren_depth == 0 {
            parts.push(&source[start..index]);
            start = index + ch.len_utf8();
        }
    }
    parts.push(&source[start..]);
    parts
}

fn parse_css_declarations(source: &str) -> Vec<CssDeclaration> {
    let mut declarations = Vec::new();
    for declaration in split_css_top_level(source, ';') {
        let Some(colon) = find_css_char(declaration, 0, ':') else {
            continue;
        };
        let name = declaration[..colon].trim().to_ascii_lowercase();
        let mut value = declaration[colon + 1..].trim().to_string();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let lower_value = value.to_ascii_lowercase();
        let important = lower_value.ends_with("!important");
        if important {
            let new_len = value.len().saturating_sub("!important".len());
            value.truncate(new_len);
            value = value.trim().to_string();
        }
        expand_css_declaration(&name, &value, important, &mut declarations);
    }
    declarations
}

fn expand_css_declaration(
    name: &str,
    value: &str,
    important: bool,
    output: &mut Vec<CssDeclaration>,
) {
    let mut push = |name: &str, value: String| {
        output.push(CssDeclaration {
            name: name.to_string(),
            value,
            important,
        });
    };

    match name {
        "margin" | "padding" | "border-width" => {
            let values = split_css_whitespace(value);
            if let Some([top, right, bottom, left]) = expand_four_sides(&values) {
                let prefix = name.trim_end_matches("-width");
                let suffix = if name == "border-width" { "-width" } else { "" };
                push(&format!("{prefix}-top{suffix}"), top);
                push(&format!("{prefix}-right{suffix}"), right);
                push(&format!("{prefix}-bottom{suffix}"), bottom);
                push(&format!("{prefix}-left{suffix}"), left);
            }
        }
        "background" => {
            push("background-color", value.to_string());
        }
        "border" => {
            push("border-width", value.to_string());
            push("border-color", value.to_string());
        }
        "text-decoration" => {
            push("text-decoration-line", value.to_string());
        }
        _ => push(name, value.to_string()),
    }
}

fn split_css_whitespace(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut paren_depth = 0u32;
    for ch in value.chars() {
        if let Some(active) = quote {
            current.push(ch);
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        if ch == '(' {
            paren_depth += 1;
            current.push(ch);
            continue;
        }
        if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            current.push(ch);
            continue;
        }
        if ch.is_whitespace() && paren_depth == 0 {
            if !current.is_empty() {
                values.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        values.push(current);
    }
    values
}

fn expand_four_sides(values: &[String]) -> Option<[String; 4]> {
    match values {
        [all] => Some([all.clone(), all.clone(), all.clone(), all.clone()]),
        [vertical, horizontal] => Some([
            vertical.clone(),
            horizontal.clone(),
            vertical.clone(),
            horizontal.clone(),
        ]),
        [top, horizontal, bottom] => Some([
            top.clone(),
            horizontal.clone(),
            bottom.clone(),
            horizontal.clone(),
        ]),
        [top, right, bottom, left, ..] => {
            Some([top.clone(), right.clone(), bottom.clone(), left.clone()])
        }
        _ => None,
    }
}

fn parse_css_selector(source: &str) -> Option<CssSelector> {
    let tokens = tokenize_selector(source);
    if tokens.is_empty() {
        return None;
    }

    let mut compounds = Vec::new();
    let mut combinators = Vec::new();
    let mut expect_compound = true;
    for token in tokens {
        match token {
            SelectorToken::Compound(value) => {
                compounds.push(parse_compound_selector(&value)?);
                expect_compound = false;
            }
            SelectorToken::Combinator(combinator) => {
                if expect_compound || compounds.is_empty() {
                    continue;
                }
                combinators.push(combinator);
                expect_compound = true;
            }
        }
    }
    while combinators.len() + 1 > compounds.len() {
        combinators.pop();
    }
    while combinators.len() + 1 < compounds.len() {
        combinators.push(CssCombinator::Descendant);
    }
    if compounds.is_empty() {
        return None;
    }

    let specificity = compounds.iter().fold(0u32, |total, compound| {
        total
            + u32::from(compound.id.is_some()) * 100
            + (compound.classes.len() + compound.attributes.len()) as u32 * 10
            + u32::from(compound.tag.is_some())
    });
    Some(CssSelector {
        compounds,
        combinators,
        specificity,
    })
}

#[derive(Debug)]
enum SelectorToken {
    Compound(String),
    Combinator(CssCombinator),
}

fn tokenize_selector(source: &str) -> Vec<SelectorToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut bracket_depth = 0u32;
    let mut paren_depth = 0u32;
    let mut quote = None;
    let mut pending_descendant = false;

    let flush = |tokens: &mut Vec<SelectorToken>, current: &mut String| {
        let value = current.trim();
        if !value.is_empty() {
            tokens.push(SelectorToken::Compound(value.to_string()));
        }
        current.clear();
    };

    for ch in source.chars() {
        if let Some(active) = quote {
            current.push(ch);
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        match ch {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            _ => {}
        }
        if bracket_depth == 0 && paren_depth == 0 && ch == '>' {
            flush(&mut tokens, &mut current);
            if matches!(tokens.last(), Some(SelectorToken::Combinator(_))) {
                tokens.pop();
            }
            tokens.push(SelectorToken::Combinator(CssCombinator::Child));
            pending_descendant = false;
        } else if bracket_depth == 0 && paren_depth == 0 && ch.is_whitespace() {
            if !current.trim().is_empty() {
                flush(&mut tokens, &mut current);
                pending_descendant = true;
            }
        } else {
            if pending_descendant {
                if !matches!(tokens.last(), Some(SelectorToken::Combinator(_))) {
                    tokens.push(SelectorToken::Combinator(CssCombinator::Descendant));
                }
                pending_descendant = false;
            }
            current.push(ch);
        }
    }
    flush(&mut tokens, &mut current);
    tokens
}

fn parse_compound_selector(source: &str) -> Option<CssCompoundSelector> {
    let mut selector = CssCompoundSelector::default();
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0usize;

    if chars.first().is_some_and(|ch| *ch == '*') {
        index += 1;
    } else if chars
        .first()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '_' || *ch == '-')
    {
        let start = index;
        while index < chars.len()
            && (chars[index].is_ascii_alphanumeric() || matches!(chars[index], '_' | '-'))
        {
            index += 1;
        }
        selector.tag = Some(chars[start..index].iter().collect::<String>().to_ascii_lowercase());
    }

    while index < chars.len() {
        match chars[index] {
            '#' | '.' => {
                let is_id = chars[index] == '#';
                index += 1;
                let start = index;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric()
                    || matches!(chars[index], '_' | '-' | '\\'))
                {
                    index += 1;
                }
                let value = chars[start..index]
                    .iter()
                    .collect::<String>()
                    .replace('\\', "")
                    .to_ascii_lowercase();
                if value.is_empty() {
                    continue;
                }
                if is_id {
                    selector.id = Some(value);
                } else {
                    selector.classes.push(value);
                }
            }
            '[' => {
                let start = index + 1;
                index += 1;
                let mut quote = None;
                while index < chars.len() {
                    let ch = chars[index];
                    if let Some(active) = quote {
                        if ch == active {
                            quote = None;
                        }
                    } else if ch == '\'' || ch == '"' {
                        quote = Some(ch);
                    } else if ch == ']' {
                        break;
                    }
                    index += 1;
                }
                let content = chars[start..index].iter().collect::<String>();
                index = (index + 1).min(chars.len());
                let mut parts = content.splitn(2, '=');
                let name = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
                let value = parts.next().map(|value| {
                    value
                        .trim()
                        .trim_matches(|ch| ch == '\'' || ch == '"')
                        .to_ascii_lowercase()
                });
                if !name.is_empty() {
                    selector.attributes.push((name, value));
                }
            }
            ':' => {
                // Pseudo-classes/elements are intentionally ignored. This keeps common
                // selectors usable without pretending to implement dynamic browser state.
                index += 1;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric() || matches!(chars[index], '-' | '_'))
                {
                    index += 1;
                }
                if index < chars.len() && chars[index] == '(' {
                    let mut depth = 1u32;
                    index += 1;
                    while index < chars.len() && depth > 0 {
                        if chars[index] == '(' {
                            depth += 1;
                        } else if chars[index] == ')' {
                            depth -= 1;
                        }
                        index += 1;
                    }
                }
            }
            _ => index += 1,
        }
    }
    Some(selector)
}

fn selector_matches(selector: &CssSelector, element: ElementRef<'_>) -> bool {
    let Some(last) = selector.compounds.last() else {
        return false;
    };
    if !compound_matches(last, &element) {
        return false;
    }

    let mut current = element;
    for index in (0..selector.compounds.len().saturating_sub(1)).rev() {
        let combinator = selector
            .combinators
            .get(index)
            .copied()
            .unwrap_or(CssCombinator::Descendant);
        let expected = &selector.compounds[index];
        match combinator {
            CssCombinator::Child => {
                let Some(parent) = parent_element(current) else {
                    return false;
                };
                if !compound_matches(expected, &parent) {
                    return false;
                }
                current = parent;
            }
            CssCombinator::Descendant => {
                let mut ancestor = parent_element(current);
                let mut found = None;
                while let Some(candidate) = ancestor {
                    if compound_matches(expected, &candidate) {
                        found = Some(candidate);
                        break;
                    }
                    ancestor = parent_element(candidate);
                }
                let Some(found) = found else {
                    return false;
                };
                current = found;
            }
        }
    }
    true
}

fn parent_element(element: ElementRef<'_>) -> Option<ElementRef<'_>> {
    let mut parent = element.parent();
    while let Some(node) = parent {
        if let Some(element) = ElementRef::wrap(node) {
            return Some(element);
        }
        parent = node.parent();
    }
    None
}

fn compound_matches(selector: &CssCompoundSelector, element: &ElementRef<'_>) -> bool {
    if let Some(tag) = &selector.tag {
        if !element.value().name().eq_ignore_ascii_case(tag) {
            return false;
        }
    }
    if let Some(id) = &selector.id {
        if !element
            .value()
            .attr("id")
            .is_some_and(|value| value.eq_ignore_ascii_case(id))
        {
            return false;
        }
    }
    if !selector.classes.is_empty() {
        let classes = element
            .value()
            .attr("class")
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>();
        if selector.classes.iter().any(|class| !classes.contains(class)) {
            return false;
        }
    }
    for (name, expected) in &selector.attributes {
        let Some(actual) = element.value().attr(name) else {
            return false;
        };
        if let Some(expected) = expected {
            if !actual.eq_ignore_ascii_case(expected) {
                return false;
            }
        }
    }
    true
}

fn apply_user_agent_style(tag: &str, style: &mut CssComputedStyle) {
    style.display = if is_inline_tag(tag) {
        HtmlDisplay::Inline
    } else {
        HtmlDisplay::Block
    };

    match tag {
        "h1" => apply_heading_defaults(style, 32.0),
        "h2" => apply_heading_defaults(style, 26.0),
        "h3" => apply_heading_defaults(style, 22.0),
        "h4" => apply_heading_defaults(style, 18.0),
        "h5" => apply_heading_defaults(style, 16.0),
        "h6" => apply_heading_defaults(style, 14.0),
        "p" | "address" | "figcaption" | "summary" | "dt" | "dd" => {
            style.block.margin.bottom = DEFAULT_PARAGRAPH_GAP;
        }
        "ul" | "ol" | "dl" => {
            style.block.margin.bottom = DEFAULT_PARAGRAPH_GAP;
            style.block.padding.left = 2.0;
        }
        "blockquote" => {
            style.block.margin.bottom = DEFAULT_PARAGRAPH_GAP;
            style.block.padding.left = 12.0;
        }
        "pre" | "table" | "figure" | "video" | "iframe" | "img" => {
            style.block.margin.bottom = DEFAULT_PARAGRAPH_GAP;
        }
        "code" | "kbd" | "samp" => style.inline.code = true,
        "small" => style.inline.font_size = Some(12.0),
        "mark" => style.inline.background_color = parse_css_color("#fff59d"),
        "summary" => style.inline.bold = true,
        _ => {}
    }
}

fn apply_heading_defaults(style: &mut CssComputedStyle, size: f32) {
    style.inline.bold = true;
    style.inline.font_size = Some(size);
    style.inline.line_height = Some(size * 1.25);
    style.block.font_size = Some(size);
    style.block.line_height = Some(size * 1.25);
    style.block.margin.bottom = 10.0;
}

fn is_inline_tag(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr" | "b" | "bdi" | "bdo" | "cite" | "code" | "data" | "del"
            | "dfn" | "em" | "font" | "i" | "ins" | "kbd" | "label" | "mark"
            | "q" | "s" | "samp" | "small" | "span" | "strike" | "strong" | "sub"
            | "sup" | "time" | "u" | "var" | "wbr" | "input" | "button"
    )
}

fn apply_semantic_inline_style(
    tag: &str,
    element: &ElementRef<'_>,
    style: &mut HtmlInlineStyle,
) {
    match tag {
        "strong" | "b" | "th" => style.bold = true,
        "em" | "i" | "cite" | "var" => style.italic = true,
        "u" | "ins" => style.underline = true,
        "s" | "strike" | "del" => style.strike = true,
        "code" | "pre" | "kbd" | "samp" => style.code = true,
        "a" => {
            style.underline = true;
            style.link = element.value().attr("href").map(ToString::to_string);
        }
        "font" => {
            if let Some(color) = element.value().attr("color").and_then(parse_css_color) {
                style.color = Some(color);
            }
        }
        _ => {}
    }
}

fn apply_css_property(
    name: &str,
    value: &str,
    style: &mut CssComputedStyle,
    options: &HtmlRenderOptions,
) {
    let lower = value.trim().to_ascii_lowercase();
    let inherited_font_size = style
        .inline
        .font_size
        .or(style.block.font_size)
        .unwrap_or(options.root_font_size.max(1.0));

    match name {
        "display" => {
            style.display = match lower.as_str() {
                "none" => HtmlDisplay::None,
                "inline" | "inline-block" | "inline-flex" => HtmlDisplay::Inline,
                _ => HtmlDisplay::Block,
            }
        }
        "visibility" if lower == "hidden" || lower == "collapse" => {
            style.display = HtmlDisplay::None;
        }
        "color" => style.inline.color = parse_css_color(value),
        "background-color" => style.block.background_color = extract_first_css_color(value),
        "font-size" => {
            if let Some(px) = parse_font_size(value, inherited_font_size, options.root_font_size) {
                style.inline.font_size = Some(px);
                style.block.font_size = Some(px);
            }
        }
        "line-height" => {
            if let Some(px) = parse_line_height(value, inherited_font_size, options.root_font_size) {
                style.inline.line_height = Some(px);
                style.block.line_height = Some(px);
            }
        }
        "font-weight" => {
            style.inline.bold = matches!(lower.as_str(), "bold" | "bolder")
                || lower.parse::<u16>().is_ok_and(|weight| weight >= 600);
        }
        "font-style" => style.inline.italic = lower.contains("italic") || lower.contains("oblique"),
        "font-family" => {
            let family = value
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches(|ch| ch == '\'' || ch == '"');
            if !family.is_empty() {
                style.inline.font_family = Some(family.to_string());
            }
        }
        "text-decoration-line" => {
            style.inline.underline = lower.contains("underline");
            style.inline.strike = lower.contains("line-through");
        }
        "text-align" => {
            style.block.text_align = match lower.as_str() {
                "center" => HtmlTextAlign::Center,
                "right" | "end" => HtmlTextAlign::Right,
                _ => HtmlTextAlign::Left,
            }
        }
        "white-space" => {
            style.inline.preserve_whitespace = matches!(
                lower.as_str(),
                "pre" | "pre-wrap" | "break-spaces"
            );
        }
        "width" => style.block.width = parse_css_length(value, inherited_font_size, options),
        "height" => style.block.height = parse_css_length(value, inherited_font_size, options),
        "min-width" => {
            style.block.min_width = parse_css_length(value, inherited_font_size, options)
        }
        "min-height" => {
            style.block.min_height = parse_css_length(value, inherited_font_size, options)
        }
        "max-width" => {
            style.block.max_width = parse_css_length(value, inherited_font_size, options)
        }
        "max-height" => {
            style.block.max_height = parse_css_length(value, inherited_font_size, options)
        }
        "margin-top" => {
            style.block.margin.top = parse_spacing_px(value, inherited_font_size, options)
        }
        "margin-right" => {
            style.block.margin.right = parse_spacing_px(value, inherited_font_size, options)
        }
        "margin-bottom" => {
            style.block.margin.bottom = parse_spacing_px(value, inherited_font_size, options)
        }
        "margin-left" => {
            style.block.margin.left = parse_spacing_px(value, inherited_font_size, options)
        }
        "padding-top" => {
            style.block.padding.top = parse_spacing_px(value, inherited_font_size, options)
        }
        "padding-right" => {
            style.block.padding.right = parse_spacing_px(value, inherited_font_size, options)
        }
        "padding-bottom" => {
            style.block.padding.bottom = parse_spacing_px(value, inherited_font_size, options)
        }
        "padding-left" => {
            style.block.padding.left = parse_spacing_px(value, inherited_font_size, options)
        }
        "border-width" | "border-top-width" | "border-right-width" | "border-bottom-width"
        | "border-left-width" => {
            style.block.border_width = parse_border_width(value, inherited_font_size, options)
        }
        "border-color" | "border-top-color" | "border-right-color" | "border-bottom-color"
        | "border-left-color" => {
            style.block.border_color = extract_first_css_color(value);
        }
        "border-radius" => {
            style.block.border_radius = parse_spacing_px(value, inherited_font_size, options)
        }
        "opacity" => {
            if let Ok(opacity) = lower.parse::<f32>() {
                let opacity = opacity.clamp(0.0, 1.0);
                style.block.opacity = opacity;
                style.inline.opacity = opacity;
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            style.block.overflow_hidden = matches!(lower.as_str(), "hidden" | "clip" | "auto");
        }
        "object-fit" => {
            style.block.object_fit = match lower.as_str() {
                "cover" => HtmlObjectFit::Cover,
                "fill" => HtmlObjectFit::Fill,
                _ => HtmlObjectFit::Contain,
            }
        }
        "list-style-type" => {
            style.block.list_style_type = Some(SharedString::from(lower));
        }
        _ => {}
    }
}

fn parse_font_size(value: &str, current: f32, root: f32) -> Option<f32> {
    match value.trim().to_ascii_lowercase().as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "smaller" => Some(current * 0.85),
        "larger" => Some(current * 1.2),
        _ => parse_absolute_css_px(value, current, root),
    }
}

fn parse_line_height(value: &str, font_size: f32, root: f32) -> Option<f32> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("normal") {
        return Some(font_size * DEFAULT_LINE_HEIGHT_RATIO);
    }
    if let Ok(multiplier) = value.parse::<f32>() {
        return Some(font_size * multiplier.max(0.0));
    }
    parse_absolute_css_px(value, font_size, root)
}

fn parse_css_length(value: &str, current_font: f32, options: &HtmlRenderOptions) -> HtmlCssLength {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto")
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("initial")
        || value.eq_ignore_ascii_case("inherit")
    {
        return HtmlCssLength::Auto;
    }
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .map(|value| HtmlCssLength::Percent((value / 100.0).max(0.0)))
            .unwrap_or(HtmlCssLength::Auto);
    }
    parse_absolute_css_px(value, current_font, options.root_font_size)
        .map(HtmlCssLength::Px)
        .unwrap_or(HtmlCssLength::Auto)
}

fn parse_spacing_px(value: &str, current_font: f32, options: &HtmlRenderOptions) -> f32 {
    if value.trim().eq_ignore_ascii_case("auto") {
        return 0.0;
    }
    parse_absolute_css_px(value, current_font, options.root_font_size)
        .unwrap_or(0.0)
        .max(0.0)
}

fn parse_absolute_css_px(value: &str, current_font: f32, root_font: f32) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    if value == "0" {
        return Some(0.0);
    }
    for (suffix, multiplier) in [
        ("px", 1.0),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
    ] {
        if let Some(number) = value.strip_suffix(suffix) {
            return number.trim().parse::<f32>().ok().map(|v| v * multiplier);
        }
    }
    if let Some(number) = value.strip_suffix("rem") {
        return number.trim().parse::<f32>().ok().map(|v| v * root_font);
    }
    if let Some(number) = value.strip_suffix("em") {
        return number.trim().parse::<f32>().ok().map(|v| v * current_font);
    }
    value.parse::<f32>().ok()
}

fn parse_border_width(value: &str, current_font: f32, options: &HtmlRenderOptions) -> f32 {
    let lower = value.to_ascii_lowercase();
    if lower.contains("thin") {
        return 1.0;
    }
    if lower.contains("medium") {
        return 2.0;
    }
    if lower.contains("thick") {
        return 3.0;
    }
    split_css_whitespace(value)
        .into_iter()
        .find_map(|part| parse_absolute_css_px(&part, current_font, options.root_font_size))
        .unwrap_or(0.0)
        .max(0.0)
}

fn extract_first_css_color(value: &str) -> Option<Hsla> {
    parse_css_color(value).or_else(|| {
        split_css_whitespace(value)
            .into_iter()
            .find_map(|part| parse_css_color(&part))
    })
}

fn parse_css_color(value: &str) -> Option<Hsla> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("inherit")
        || trimmed.eq_ignore_ascii_case("initial")
        || trimmed.eq_ignore_ascii_case("currentcolor")
        || trimmed.eq_ignore_ascii_case("transparent")
    {
        return None;
    }
    let parsed = csscolorparser::parse(trimmed).ok()?;
    if parsed.a <= 0.01 {
        return None;
    }
    Some(Hsla::from(Rgba {
        r: parsed.r as f32,
        g: parsed.g as f32,
        b: parsed.b as f32,
        a: parsed.a as f32,
    }))
}

fn resolve_css_vars(
    value: &str,
    custom_properties: &HashMap<String, String>,
    remaining_depth: usize,
) -> String {
    if remaining_depth == 0 || !value.contains("var(") {
        return value.to_string();
    }
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while let Some(relative_start) = value[cursor..].find("var(") {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let open = start + 3;
        let Some(close) = find_matching_parenthesis(value, open) else {
            output.push_str(&value[start..]);
            return output;
        };
        let content = &value[open + 1..close];
        let mut parts = split_css_top_level(content, ',').into_iter();
        let name = parts.next().unwrap_or_default().trim();
        let fallback = parts.next().unwrap_or_default().trim();
        let replacement = custom_properties
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback);
        output.push_str(&resolve_css_vars(
            replacement,
            custom_properties,
            remaining_depth - 1,
        ));
        cursor = close + 1;
    }
    output.push_str(&value[cursor..]);
    output
}

fn find_matching_parenthesis(value: &str, open: usize) -> Option<usize> {
    let mut depth = 0u32;
    for (offset, ch) in value[open..].char_indices() {
        let index = open + offset;
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn resolve_resource_url(base_url: Option<&str>, resource: &str) -> String {
    let resource = resource.trim();
    if resource.is_empty()
        || resource.contains("://")
        || resource.starts_with("data:")
        || resource.starts_with("file:")
        || resource.starts_with("blob:")
    {
        return resource.to_string();
    }
    let Some(base) = base_url.map(str::trim).filter(|base| !base.is_empty()) else {
        return resource.to_string();
    };
    if resource.starts_with("//") {
        let scheme = base
            .split_once("://")
            .map(|(scheme, _)| scheme)
            .unwrap_or("https");
        return format!("{scheme}:{resource}");
    }

    let clean_base = base
        .split('#')
        .next()
        .unwrap_or(base)
        .split('?')
        .next()
        .unwrap_or(base);
    if let Some((scheme, rest)) = clean_base.split_once("://") {
        let (authority, path) = rest
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((rest, "/".to_string()));
        if resource.starts_with('/') {
            return format!("{scheme}://{authority}{resource}");
        }
        let directory = if path.ends_with('/') {
            path
        } else {
            path.rsplit_once('/')
                .map(|(directory, _)| format!("{directory}/"))
                .unwrap_or_else(|| "/".to_string())
        };
        return format!(
            "{scheme}://{authority}/{}{}",
            directory.trim_matches('/'),
            if directory == "/" { "" } else { "/" },
        ) + resource.trim_start_matches('/');
    }

    let directory = if clean_base.ends_with('/') {
        clean_base
    } else {
        clean_base
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or(clean_base)
    };
    format!(
        "{}/{}",
        directory.trim_end_matches('/'),
        resource.trim_start_matches('/')
    )
}

// ------------------------------ GPUI renderer ------------------------------

fn render_block(
    block: &HtmlBlock,
    style: &HtmlBlockStyle,
    colors: &ThemeColors,
    image_cache: Option<&HashMap<SharedString, Arc<RenderImage>>>,
) -> AnyElement {
    let default_font_size = style.font_size.unwrap_or(14.0).max(1.0);
    let default_line_height = style
        .line_height
        .unwrap_or(default_font_size * DEFAULT_LINE_HEIGHT_RATIO)
        .max(default_font_size);

    let inner = match block {
        HtmlBlock::Heading { level, spans } => {
            let fallback_size = match *level {
                1 => 32.0,
                2 => 26.0,
                3 => 22.0,
                4 => 18.0,
                5 => 16.0,
                _ => 14.0,
            };
            let font_size = style.font_size.unwrap_or(fallback_size);
            let line_height = style.line_height.unwrap_or(font_size * 1.25);
            div()
                .w_full()
                .child(render_inline_with_links(
                    spans,
                    colors,
                    px(font_size),
                    px(line_height),
                    true,
                ))
                .into_any_element()
        }
        HtmlBlock::Paragraph { spans } => div()
            .w_full()
            .child(render_inline_with_links(
                spans,
                colors,
                px(default_font_size),
                px(default_line_height),
                false,
            ))
            .into_any_element(),
        HtmlBlock::List { ordered, items } => render_list_items(
            items,
            *ordered,
            0,
            style.list_style_type.as_ref().map(SharedString::as_str),
            colors,
            px(default_font_size),
            px(default_line_height),
        )
            .into_any_element(),
        HtmlBlock::Quote { spans } => div()
            .w_full()
            .pl(px(12.0))
            .border_l_2()
            .border_color(style.border_color.unwrap_or(colors.accent))
            .child(render_inline_with_links(
                spans,
                colors,
                px(default_font_size),
                px(default_line_height),
                false,
            ))
            .into_any_element(),
        HtmlBlock::CodeBlock { code } => {
            let code_str = code.to_string();
            let code_len = code_str.len();
            div()
                .w_full()
                .rounded(px(style.border_radius.max(10.0)))
                .bg(style.background_color.unwrap_or(Hsla {
                    a: 0.08,
                    ..colors.surface
                }))
                .border_1()
                .border_color(style.border_color.unwrap_or(colors.border))
                .overflow_hidden()
                .child(
                    div()
                        .w_full()
                        .p(px(16.0))
                        .text_size(px(style.font_size.unwrap_or(13.0)))
                        .line_height(px(style.line_height.unwrap_or(21.0)))
                        .child(
                            StyledText::new(SharedString::from(code_str)).with_runs(vec![TextRun {
                                len: code_len,
                                font: Font {
                                    family: "Cascadia Mono".into(),
                                    features: FontFeatures::default(),
                                    fallbacks: None,
                                    weight: FontWeight::NORMAL,
                                    style: FontStyle::Normal,
                                },
                                color: colors.text_primary,
                                background_color: None,
                                background_corner_radius: None,
                                background_padding: None,
                                underline: None,
                                strikethrough: None,
                            }]),
                        ),
                )
                .into_any_element()
        }
        HtmlBlock::Image { src, alt } => {
            let image_source = image_cache
                .and_then(|cache| cache.get(src))
                .cloned()
                .map(ImageSource::from)
                .unwrap_or_else(|| ImageSource::from(src.clone()));
            let object_fit = match style.object_fit {
                HtmlObjectFit::Cover => ObjectFit::Cover,
                HtmlObjectFit::Fill => ObjectFit::Fill,
                HtmlObjectFit::Contain => ObjectFit::Contain,
            };
            let block = div()
                .w_full()
                .rounded(px(style.border_radius.max(12.0)))
                .overflow_hidden()
                .when(style.border_width > 0.0 || style.border_color.is_some(), |this| {
                    this.border_1()
                        .border_color(style.border_color.unwrap_or(colors.border))
                })
                .bg(style.background_color.unwrap_or(Hsla {
                    a: 0.20,
                    ..colors.surface
                }))
                .child(
                    img(image_source)
                        .w_full()
                        .max_w_full()
                        .max_h(px(match style.max_height {
                            HtmlCssLength::Px(value) => value,
                            _ => 520.0,
                        }))
                        .object_fit(object_fit),
                );

            if alt.trim().is_empty() {
                block.into_any_element()
            } else {
                block
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .text_size(px(12.0))
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
        } => render_video_block(src, title, provider, style, colors),
        HtmlBlock::Table { headers, rows } => {
            render_table(headers, rows, style, colors).into_any_element()
        }
        HtmlBlock::Group { blocks, styles } => {
            let mut column = div().w_full().flex().flex_col();
            for (index, child) in blocks.iter().enumerate() {
                let child_style = styles.get(index).cloned().unwrap_or_default();
                column = column.child(render_block(child, &child_style, colors, image_cache));
            }
            column.into_any_element()
        }
        HtmlBlock::Rule => div()
            .h(px(style.border_width.max(1.0)))
            .w_full()
            .bg(style.border_color.unwrap_or(colors.border))
            .into_any_element(),
    };

    wrap_block(inner, style, colors)
}

fn wrap_block(
    inner: AnyElement,
    style: &HtmlBlockStyle,
    colors: &ThemeColors,
) -> AnyElement {
    let mut wrapper = div().w_full().child(inner);
    wrapper = apply_block_size(wrapper, style);

    if style.margin.top > 0.0 {
        wrapper = wrapper.mt(px(style.margin.top));
    }
    if style.margin.right > 0.0 {
        wrapper = wrapper.mr(px(style.margin.right));
    }
    if style.margin.bottom > 0.0 {
        wrapper = wrapper.mb(px(style.margin.bottom));
    }
    if style.margin.left > 0.0 {
        wrapper = wrapper.ml(px(style.margin.left));
    }
    if style.padding.top > 0.0 {
        wrapper = wrapper.pt(px(style.padding.top));
    }
    if style.padding.right > 0.0 {
        wrapper = wrapper.pr(px(style.padding.right));
    }
    if style.padding.bottom > 0.0 {
        wrapper = wrapper.pb(px(style.padding.bottom));
    }
    if style.padding.left > 0.0 {
        wrapper = wrapper.pl(px(style.padding.left));
    }
    if let Some(background) = style.background_color {
        wrapper = wrapper.bg(background);
    }
    if style.border_width > 0.0 {
        wrapper = wrapper
            .border_1()
            .border_color(style.border_color.unwrap_or(colors.border));
    }
    if style.border_radius > 0.0 {
        wrapper = wrapper.rounded(px(style.border_radius));
    }
    if style.overflow_hidden {
        wrapper = wrapper.overflow_hidden();
    }
    if style.opacity < 0.999 {
        wrapper = wrapper.opacity(style.opacity.clamp(0.0, 1.0));
    }
    wrapper = match style.text_align {
        HtmlTextAlign::Center => wrapper.text_center(),
        HtmlTextAlign::Right => wrapper.text_right(),
        HtmlTextAlign::Left => wrapper.text_left(),
    };
    wrapper.into_any_element()
}

fn apply_block_size(mut block: Div, style: &HtmlBlockStyle) -> Div {
    block = apply_width(block, style.width, SizeProperty::Preferred);
    block = apply_width(block, style.min_width, SizeProperty::Minimum);
    block = apply_width(block, style.max_width, SizeProperty::Maximum);
    block = apply_height(block, style.height, SizeProperty::Preferred);
    block = apply_height(block, style.min_height, SizeProperty::Minimum);
    apply_height(block, style.max_height, SizeProperty::Maximum)
}

#[derive(Clone, Copy)]
enum SizeProperty {
    Preferred,
    Minimum,
    Maximum,
}

fn apply_width(block: Div, value: HtmlCssLength, property: SizeProperty) -> Div {
    match (value, property) {
        (HtmlCssLength::Auto, _) => block,
        (HtmlCssLength::Px(value), SizeProperty::Preferred) => block.w(px(value.max(0.0))),
        (HtmlCssLength::Px(value), SizeProperty::Minimum) => block.min_w(px(value.max(0.0))),
        (HtmlCssLength::Px(value), SizeProperty::Maximum) => block.max_w(px(value.max(0.0))),
        (HtmlCssLength::Percent(value), SizeProperty::Preferred) => block.w(relative(value)),
        (HtmlCssLength::Percent(value), SizeProperty::Minimum) => block.min_w(relative(value)),
        (HtmlCssLength::Percent(value), SizeProperty::Maximum) => block.max_w(relative(value)),
    }
}

fn apply_height(block: Div, value: HtmlCssLength, property: SizeProperty) -> Div {
    match (value, property) {
        (HtmlCssLength::Auto, _) => block,
        (HtmlCssLength::Px(value), SizeProperty::Preferred) => block.h(px(value.max(0.0))),
        (HtmlCssLength::Px(value), SizeProperty::Minimum) => block.min_h(px(value.max(0.0))),
        (HtmlCssLength::Px(value), SizeProperty::Maximum) => block.max_h(px(value.max(0.0))),
        (HtmlCssLength::Percent(value), SizeProperty::Preferred) => block.h(relative(value)),
        (HtmlCssLength::Percent(value), SizeProperty::Minimum) => block.min_h(relative(value)),
        (HtmlCssLength::Percent(value), SizeProperty::Maximum) => block.max_h(relative(value)),
    }
}

fn render_video_block(
    src: &SharedString,
    title: &SharedString,
    provider: &SharedString,
    style: &HtmlBlockStyle,
    colors: &ThemeColors,
) -> AnyElement {
    let card = div()
        .w_full()
        .rounded(px(style.border_radius.max(18.0)))
        .overflow_hidden()
        .cursor_pointer()
        .border_1()
        .border_color(style.border_color.unwrap_or(Hsla {
            a: 0.08,
            ..colors.border
        }));
    let card = if let Some(background) = style.background_color {
        card.bg(background)
    } else {
        card.bg(linear_gradient(
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
    };
    card.child(
        div()
            .w_full()
            .p(px(18.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .w_full()
                    .h(px(180.0))
                    .rounded(px(16.0))
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
                            .size(px(56.0))
                            .rounded(px(999.0))
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
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.text_muted)
                                    .child(provider.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.text_primary)
                                    .child(title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.text_secondary)
                                    .child("点击后将在浏览器中播放"),
                            ),
                    )
                    .child(
                        div()
                            .h(px(40.0))
                            .px(px(14.0))
                            .rounded(px(12.0))
                            .bg(colors.accent)
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(12.0))
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
            move |_event, _window, cx| cx.open_url(src.as_ref())
        })
        .into_any_element()
}

fn render_table(
    headers: &[Vec<HtmlInline>],
    rows: &[Vec<Vec<HtmlInline>>],
    style: &HtmlBlockStyle,
    colors: &ThemeColors,
) -> Div {
    let col_count = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if col_count == 0 {
        return div();
    }

    let font_size = px(style.font_size.unwrap_or(13.0));
    let line_height = px(style.line_height.unwrap_or(20.0));
    let mut table = div()
        .w_full()
        .rounded(px(style.border_radius.max(10.0)))
        .border_1()
        .border_color(style.border_color.unwrap_or(colors.border))
        .overflow_hidden()
        .flex()
        .flex_col();

    if !headers.is_empty() {
        let mut header_row = div().flex().bg(Hsla {
            a: 0.15,
            ..colors.accent
        });
        for column in 0..col_count {
            let cell = headers.get(column).cloned().unwrap_or_default();
            header_row = header_row.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .when(column + 1 < col_count, |this| {
                        this.border_r_1().border_color(Hsla {
                            a: 0.18,
                            ..colors.border
                        })
                    })
                    .child(render_inline_with_links(
                        &cell,
                        colors,
                        font_size,
                        line_height,
                        true,
                    )),
            );
        }
        table = table.child(header_row);
    }

    for (row_index, row) in rows.iter().enumerate() {
        let mut data_row = div()
            .flex()
            .border_t_1()
            .border_color(colors.border)
            .bg(Hsla {
                a: if row_index % 2 == 0 { 0.0 } else { 0.04 },
                ..colors.surface
            });
        for column in 0..col_count {
            let cell = row.get(column).cloned().unwrap_or_default();
            data_row = data_row.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(12.0))
                    .py(px(9.0))
                    .when(column + 1 < col_count, |this| {
                        this.border_r_1().border_color(Hsla {
                            a: 0.10,
                            ..colors.border
                        })
                    })
                    .child(render_inline_with_links(
                        &cell,
                        colors,
                        font_size,
                        line_height,
                        false,
                    )),
            );
        }
        table = table.child(data_row);
    }
    table
}

fn render_list_items(
    items: &[HtmlListItem],
    ordered: bool,
    depth: usize,
    list_style_type: Option<&str>,
    colors: &ThemeColors,
    font_size: Pixels,
    line_height: Pixels,
) -> Div {
    let mut list = div().w_full().flex().flex_col().gap(px(6.0));
    for (index, item) in items.iter().enumerate() {
        let marker = list_marker(ordered, index, depth, list_style_type);
        let marker_width = if marker.is_empty() { 0.0 } else { 24.0 };
        let mut item_column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .gap(px(if marker.is_empty() { 0.0 } else { 8.0 }))
                    .when(!marker.is_empty(), |this| {
                        this.child(
                            div()
                                .w(px(marker_width))
                                .flex_none()
                                .text_size(font_size)
                                .line_height(line_height)
                                .text_color(colors.text_secondary)
                                .child(marker),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(render_inline_with_links(
                                &item.spans,
                                colors,
                                font_size,
                                line_height,
                                false,
                            )),
                    ),
            );

        if let Some((sub_ordered, sub_items)) = &item.sub_list {
            item_column = item_column.child(
                div().pl(px(marker_width + 8.0)).child(render_list_items(
                    sub_items,
                    *sub_ordered,
                    depth + 1,
                    list_style_type,
                    colors,
                    font_size,
                    line_height,
                )),
            );
        }
        list = list.child(item_column);
    }
    list
}

fn list_marker(
    ordered: bool,
    index: usize,
    depth: usize,
    list_style_type: Option<&str>,
) -> String {
    let style = list_style_type.unwrap_or(if ordered { "decimal" } else { "disc" });
    match style {
        "none" => String::new(),
        "circle" => "◦".to_string(),
        "square" => "▪".to_string(),
        "lower-alpha" | "lower-latin" => {
            format!("{}.", alphabetic_marker(index, false))
        }
        "upper-alpha" | "upper-latin" => {
            format!("{}.", alphabetic_marker(index, true))
        }
        "decimal" | "decimal-leading-zero" if ordered => {
            if style == "decimal-leading-zero" && index < 9 {
                format!("0{}.", index + 1)
            } else {
                format!("{}.", index + 1)
            }
        }
        _ if ordered => format!("{}.", index + 1),
        _ => match depth {
            0 => "•".to_string(),
            1 => "◦".to_string(),
            _ => "▪".to_string(),
        },
    }
}

fn alphabetic_marker(mut index: usize, uppercase: bool) -> String {
    index += 1;
    let mut output = String::new();
    while index > 0 {
        let digit = (index - 1) % 26;
        let base = if uppercase { b'A' } else { b'a' };
        output.insert(0, (base + digit as u8) as char);
        index = (index - 1) / 26;
    }
    output
}

fn adapt_html_color_contrast(mut color: Hsla, surface: Hsla, text_primary: Hsla) -> Hsla {
    color.a = color.a.max(0.85);
    let background_lightness = surface.l;
    if background_lightness > 0.5 {
        if color.l > 0.80 {
            text_primary
        } else if color.l > 0.45 {
            color.l = 0.35;
            color
        } else {
            color
        }
    } else if color.l < 0.20 {
        text_primary
    } else if color.l < 0.55 {
        color.l = 0.75;
        color
    } else {
        color
    }
}

/// Groups spans only when their text metrics and link target match. This keeps
/// shaping/batching efficient while still allowing CSS font-size, family,
/// background, and line-height changes inside a paragraph.
fn render_inline_with_links(
    spans: &[HtmlInline],
    colors: &ThemeColors,
    font_size: Pixels,
    line_height: Pixels,
    is_heading: bool,
) -> AnyElement {
    if spans.is_empty() {
        return div().into_any_element();
    }

    let groups = group_inline_spans(spans);
    let mut row = div().w_full().flex().flex_wrap();
    for group in groups {
        row = row.children(render_inline_group(
            &group,
            colors,
            font_size,
            line_height,
            is_heading,
        ));
    }
    row.into_any_element()
}

fn group_inline_spans(spans: &[HtmlInline]) -> Vec<Vec<HtmlInline>> {
    fn compatible(left: &HtmlInlineStyle, right: &HtmlInlineStyle) -> bool {
        left.link == right.link
            && left.font_size == right.font_size
            && left.line_height == right.line_height
            && left.font_family == right.font_family
            && left.preserve_whitespace == right.preserve_whitespace
    }

    let mut groups: Vec<Vec<HtmlInline>> = Vec::new();
    for span in spans {
        if groups
            .last()
            .and_then(|group| group.last())
            .is_some_and(|previous| compatible(&previous.style, &span.style))
        {
            groups.last_mut().unwrap().push(span.clone());
        } else {
            groups.push(vec![span.clone()]);
        }
    }
    groups
}

fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4e00}'..='\u{9fff}' |
        '\u{3400}'..='\u{4dbf}' |
        '\u{f900}'..='\u{faff}'
    )
}

fn split_into_words_and_whitespaces(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    enum Kind {
        Whitespace,
        Cjk,
        Other,
    }
    let mut current_kind = None;

    for ch in text.chars() {
        let kind = if ch.is_whitespace() {
            Kind::Whitespace
        } else if is_cjk(ch) {
            Kind::Cjk
        } else {
            Kind::Other
        };

        match (&mut current_kind, kind) {
            (None, k) => {
                current.push(ch);
                current_kind = Some(k);
            }
            (Some(Kind::Whitespace), Kind::Whitespace) => {
                current.push(ch);
            }
            (Some(Kind::Other), Kind::Other) => {
                current.push(ch);
            }
            (Some(_), k) => {
                pieces.push(current);
                current = String::new();
                current.push(ch);
                current_kind = Some(k);
            }
        }
    }

    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

fn render_inline_group(
    spans: &[HtmlInline],
    colors: &ThemeColors,
    default_font_size: Pixels,
    default_line_height: Pixels,
    is_heading: bool,
) -> Vec<AnyElement> {
    let first = &spans[0].style;
    let font_size = first.font_size.map(px).unwrap_or(default_font_size);
    let line_height = first.line_height.map(px).unwrap_or(default_line_height);
    let link = first.link.clone();

    if first.preserve_whitespace {
        let mut element = div()
            .text_size(font_size)
            .line_height(line_height)
            .whitespace_nowrap()
            .when(link.is_some(), |this| this.cursor_pointer())
            .child(render_inline_text(spans, colors, is_heading));

        if let Some(url) = &link {
            let url = url.clone();
            element = element.on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.open_url(&url);
            });
        }
        vec![element.into_any_element()]
    } else {
        let mut elements = Vec::new();
        for span in spans {
            let pieces = split_into_words_and_whitespaces(&span.text);
            for piece in pieces {
                let single_span = HtmlInline {
                    text: piece,
                    style: span.style.clone(),
                };
                let mut element = div()
                    .flex_none()
                    .text_size(font_size)
                    .line_height(line_height)
                    .whitespace_normal()
                    .when(link.is_some(), |this| this.cursor_pointer())
                    .child(render_inline_text(&[single_span], colors, is_heading));

                if let Some(url) = &link {
                    let url = url.clone();
                    element = element.on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                        cx.open_url(&url);
                    });
                }
                elements.push(element.into_any_element());
            }
        }
        elements
    }
}

fn render_inline_text(
    spans: &[HtmlInline],
    colors: &ThemeColors,
    is_heading: bool,
) -> StyledText {
    let mut combined_text = String::new();
    let mut runs = Vec::new();

    for span in spans {
        let text = span.text.replace("\r\n", "\n");
        if text.is_empty() {
            continue;
        }
        let mut color = span
            .style
            .color
            .map(|value| adapt_html_color_contrast(value, colors.surface, colors.text_primary))
            .unwrap_or_else(|| {
                if span.style.link.is_some() {
                    colors.accent
                } else if is_heading || span.style.bold {
                    colors.text_primary
                } else {
                    colors.text_secondary
                }
            });
        color.a *= span.style.opacity.clamp(0.0, 1.0);

        let background = span.style.background_color.or_else(|| {
            span.style.code.then_some(Hsla {
                a: 0.18,
                ..colors.accent
            })
        });
        runs.push(TextRun {
            len: text.len(),
            font: Font {
                family: span
                    .style
                    .font_family
                    .clone()
                    .unwrap_or_else(|| {
                        if span.style.code {
                            "Cascadia Mono".to_string()
                        } else {
                            "HarmonyOS Sans".to_string()
                        }
                    })
                    .into(),
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
            background_color: background,
            background_corner_radius: None,
            background_padding: None,
            underline: (span.style.underline || span.style.link.is_some()).then_some(
                UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(color),
                    wavy: false,
                },
            ),
            strikethrough: span.style.strike.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
        });
        combined_text.push_str(&text);
    }

    StyledText::new(SharedString::from(combined_text)).with_runs(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_class_and_descendant_selectors_are_cascaded() {
        let html = r#"
            <style>
                .card { padding: 12px; background-color: #112233; }
                .card strong { color: rgb(255, 0, 0); }
            </style>
            <div class="card"><p>Hello <strong>world</strong></p></div>
        "#;
        let document = parse_html_document(html);
        assert_eq!(document.blocks.len(), 1);
        assert!(document.block_styles[0].padding.top >= 12.0);
        let HtmlBlock::Group { blocks, .. } = &document.blocks[0] else {
            panic!("expected styled group");
        };
        let HtmlBlock::Paragraph { spans } = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(spans.iter().any(|span| span.style.color.is_some()));
    }

    #[test]
    fn stylesheet_links_are_discovered_without_network_io() {
        let requests = discover_html_stylesheets(
            r#"<link rel="stylesheet" href="assets/site.css">"#,
            Some("https://example.com/docs/page.html"),
        );
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].href.as_ref(),
            "https://example.com/docs/assets/site.css"
        );
    }

    #[test]
    fn display_none_removes_elements() {
        let document = parse_html_document(
            r#"<style>.hidden { display: none }</style><p>A</p><p class="hidden">B</p>"#,
        );
        assert_eq!(document.blocks.len(), 1);
    }

    #[test]
    fn test_split_into_words_and_whitespaces() {
        let text = "Hello, world! 如图html";
        let pieces = split_into_words_and_whitespaces(text);
        assert_eq!(
            pieces,
            vec![
                "Hello,".to_string(),
                " ".to_string(),
                "world!".to_string(),
                " ".to_string(),
                "如".to_string(),
                "图".to_string(),
                "html".to_string()
            ]
        );
    }
}
