use crate::ui::animation::request_animation_frame_if;
use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const GLYPH_TILE_SIZE: f32 = 32.0;
const GLYPH_SPRITE_SIZE: f32 = 512.0;
const GLYPH_SHEET_E0: &str = "images/minecraft/glyph_E0.png";
const GLYPH_SHEET_E1: &str = "images/minecraft/glyph_E1.png";
const OBFUSCATED_FRAME: Duration = Duration::from_millis(16);
const PARSED_TEXT_CACHE_LIMIT: usize = 512;
static PARSED_TEXT_CACHE: Lazy<Mutex<HashMap<ParsedTextCacheKey, ParsedMinecraftText>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy, Debug, Default)]
struct MinecraftTextStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    obfuscated: bool,
}

#[derive(Clone, Debug)]
struct MinecraftTextRunSpec {
    start: usize,
    end: usize,
    color: Hsla,
    style: MinecraftTextStyle,
}

#[derive(Clone, Debug)]
struct ParsedMinecraftText {
    text: String,
    runs: Vec<MinecraftTextRunSpec>,
    has_glyphs: bool,
    has_obfuscated: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ParsedTextCacheKey {
    text: String,
    default_color: [u16; 4],
}

#[derive(Clone, Copy, Debug)]
struct InlinePieceStyle {
    color: Hsla,
    style: MinecraftTextStyle,
}

#[derive(Clone, Debug)]
enum InlinePieceKind {
    Text(SharedString),
    Glyph(u32),
    Newline,
}

#[derive(Clone, Debug)]
struct InlinePiece {
    kind: InlinePieceKind,
    style: InlinePieceStyle,
}

#[derive(IntoElement)]
pub struct MinecraftFormattedText {
    text: SharedString,
    font_size: Pixels,
    line_height: DefiniteLength,
    default_color: Hsla,
    wrap: bool,
    animate_obfuscated: bool,
}

impl MinecraftFormattedText {
    pub fn new(text: impl Into<SharedString>, colors: &ThemeColors) -> Self {
        Self {
            text: text.into(),
            font_size: px(13.0),
            line_height: relative(1.4),
            default_color: colors.text_primary,
            wrap: true,
            animate_obfuscated: true,
        }
    }

    pub fn text_size(mut self, font_size: Pixels) -> Self {
        self.font_size = font_size;
        self
    }

    pub fn line_height(mut self, line_height: DefiniteLength) -> Self {
        self.line_height = line_height;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.default_color = color;
        self
    }

    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn animate_obfuscated(mut self, animate: bool) -> Self {
        self.animate_obfuscated = animate;
        self
    }
}

impl RenderOnce for MinecraftFormattedText {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut parsed = cached_minecraft_formatted_text(self.text.as_ref(), self.default_color);
        if parsed.text.is_empty() {
            return div().into_any_element();
        }

        if parsed.has_obfuscated && self.animate_obfuscated {
            request_animation_frame_if(window, true);
            let frame = obfuscated_frame_tick();
            parsed = apply_obfuscated_frame(&parsed, frame);
        }

        if parsed.has_glyphs {
            return render_glyph_text(self, &parsed).into_any_element();
        }

        let runs = build_text_runs(&parsed.runs);

        let mut container = div()
            .text_size(self.font_size)
            .line_height(self.line_height);

        container = if self.wrap {
            container.w_full().whitespace_normal()
        } else {
            container.whitespace_nowrap()
        };

        container
            .child(StyledText::new(SharedString::from(parsed.text)).with_runs(runs))
            .into_any_element()
    }
}

fn render_glyph_text(component: MinecraftFormattedText, parsed: &ParsedMinecraftText) -> Div {
    let glyph_size = px((component.font_size / px(1.0)).max(14.0));
    let line_height = component.line_height;
    let pieces = build_inline_pieces(parsed);

    let mut root = div()
        .text_size(component.font_size)
        .line_height(line_height)
        .flex()
        .flex_col();

    if component.wrap {
        root = root.w_full();
    }

    let mut line = div().flex().items_center().line_height(line_height);
    if component.wrap {
        line = line.w_full().flex_wrap();
    }

    for piece in pieces {
        match piece.kind {
            InlinePieceKind::Newline => {
                root = root.child(line);
                line = div().flex().items_center().line_height(line_height);
                if component.wrap {
                    line = line.w_full().flex_wrap();
                }
            }
            InlinePieceKind::Text(text) => {
                line = line.child(render_text_piece(
                    text,
                    piece.style,
                    component.font_size,
                    line_height,
                ));
            }
            InlinePieceKind::Glyph(code_point) => {
                line = line.child(render_glyph_piece(code_point, glyph_size));
            }
        }
    }

    root.child(line)
}

fn render_text_piece(
    text: SharedString,
    piece_style: InlinePieceStyle,
    font_size: Pixels,
    line_height: DefiniteLength,
) -> AnyElement {
    let runs = vec![make_text_run(text.len(), piece_style)];

    div()
        .flex_none()
        .text_size(font_size)
        .line_height(line_height)
        .child(StyledText::new(text).with_runs(runs))
        .into_any_element()
}

fn render_glyph_piece(code_point: u32, glyph_size: Pixels) -> AnyElement {
    let Some((sheet_path, row, column)) = glyph_sprite_info(code_point) else {
        return div().into_any_element();
    };

    let scale = (glyph_size / px(1.0)) / GLYPH_TILE_SIZE;
    let sprite_size = px(GLYPH_SPRITE_SIZE * scale);
    let offset_x = px(-(column as f32) * GLYPH_TILE_SIZE * scale);
    let offset_y = px(-(row as f32) * GLYPH_TILE_SIZE * scale);

    div()
        .relative()
        .overflow_hidden()
        .flex_none()
        .w(glyph_size)
        .h(glyph_size)
        .child(
            img(sheet_path)
                .absolute()
                .left(offset_x)
                .top(offset_y)
                .w(sprite_size)
                .h(sprite_size),
        )
        .into_any_element()
}

fn build_inline_pieces(parsed: &ParsedMinecraftText) -> Vec<InlinePiece> {
    let mut pieces = Vec::new();
    for run in &parsed.runs {
        let style = InlinePieceStyle {
            color: run.color,
            style: run.style,
        };
        let text = &parsed.text[run.start..run.end];
        let mut buffer = String::new();
        let mut whitespace = String::new();

        for ch in text.chars() {
            if ch == '\n' {
                flush_inline_buffer(&mut pieces, &mut buffer, style);
                flush_inline_buffer(&mut pieces, &mut whitespace, style);
                pieces.push(InlinePiece {
                    kind: InlinePieceKind::Newline,
                    style,
                });
                continue;
            }

            if let Some(code_point) = glyph_code_point(ch) {
                flush_inline_buffer(&mut pieces, &mut buffer, style);
                flush_inline_buffer(&mut pieces, &mut whitespace, style);
                pieces.push(InlinePiece {
                    kind: InlinePieceKind::Glyph(code_point),
                    style,
                });
                continue;
            }

            if ch.is_whitespace() {
                flush_inline_buffer(&mut pieces, &mut buffer, style);
                whitespace.push(ch);
                continue;
            }

            flush_inline_buffer(&mut pieces, &mut whitespace, style);
            buffer.push(ch);
        }

        flush_inline_buffer(&mut pieces, &mut buffer, style);
        flush_inline_buffer(&mut pieces, &mut whitespace, style);
    }

    pieces
}

fn flush_inline_buffer(
    pieces: &mut Vec<InlinePiece>,
    buffer: &mut String,
    style: InlinePieceStyle,
) {
    if buffer.is_empty() {
        return;
    }

    pieces.push(InlinePiece {
        kind: InlinePieceKind::Text(SharedString::from(std::mem::take(buffer))),
        style,
    });
}

fn contains_glyphs(text: &str) -> bool {
    text.chars().any(|ch| glyph_code_point(ch).is_some())
}

fn glyph_code_point(ch: char) -> Option<u32> {
    let code_point = ch as u32;
    if is_glyph_code_point(code_point) {
        Some(code_point)
    } else {
        None
    }
}

fn is_glyph_code_point(code_point: u32) -> bool {
    (0xE000..=0xE0FF).contains(&code_point) || (0xE100..=0xE1FF).contains(&code_point)
}

fn glyph_sprite_info(code_point: u32) -> Option<(SharedString, u32, u32)> {
    if !is_glyph_code_point(code_point) {
        return None;
    }

    let index = code_point & 0xFF;
    let row = index / 16;
    let column = index % 16;
    let sheet_path = if (code_point & 0xFF00) == 0xE000 {
        SharedString::from(GLYPH_SHEET_E0)
    } else {
        SharedString::from(GLYPH_SHEET_E1)
    };

    Some((sheet_path, row, column))
}

fn cached_minecraft_formatted_text(input: &str, default_color: Hsla) -> ParsedMinecraftText {
    let key = ParsedTextCacheKey {
        text: input.to_string(),
        default_color: color_cache_key(default_color),
    };

    if let Ok(cache) = PARSED_TEXT_CACHE.lock()
        && let Some(parsed) = cache.get(&key)
    {
        return parsed.clone();
    }

    let parsed = parse_minecraft_formatted_text(input, default_color);
    if let Ok(mut cache) = PARSED_TEXT_CACHE.lock() {
        if cache.len() >= PARSED_TEXT_CACHE_LIMIT
            && let Some(first_key) = cache.keys().next().cloned()
        {
            cache.remove(&first_key);
        }
        cache.insert(key, parsed.clone());
    }
    parsed
}

fn color_cache_key(color: Hsla) -> [u16; 4] {
    let rgba = Rgba::from(color);
    [
        (rgba.r.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16,
        (rgba.g.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16,
        (rgba.b.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16,
        (rgba.a.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16,
    ]
}

fn parse_minecraft_formatted_text(input: &str, default_color: Hsla) -> ParsedMinecraftText {
    let mut output = String::with_capacity(input.len());
    let mut runs = Vec::new();
    let mut style = MinecraftTextStyle::default();
    let mut current_color = default_color;
    let mut run_start = 0usize;
    let mut has_glyphs = false;
    let mut has_obfuscated = false;

    let chars = input.char_indices().collect::<Vec<_>>();
    let mut index = 0usize;
    while index < chars.len() {
        let (_, ch) = chars[index];
        if ch == '§' || ch == '\u{00A7}' {
            if index + 1 >= chars.len() {
                output.push(ch);
                index += 1;
                continue;
            }

            let next = chars[index + 1].1.to_ascii_lowercase();
            let current_len = output.len();
            if current_len > run_start {
                runs.push(MinecraftTextRunSpec {
                    start: run_start,
                    end: current_len,
                    color: current_color,
                    style,
                });
            }

            match next {
                '0'..='9'
                | 'a'..='f'
                | 'g'
                | 'h'
                | 'i'
                | 'j'
                | 'm'
                | 'n'
                | 'p'
                | 'q'
                | 's'
                | 't'
                | 'u'
                | 'v' => {
                    current_color =
                        minecraft_format_color(next, default_color).unwrap_or(default_color);
                    style = MinecraftTextStyle::default();
                }
                'r' => {
                    current_color = default_color;
                    style = MinecraftTextStyle::default();
                }
                'k' => {
                    style.obfuscated = true;
                    has_obfuscated = true;
                }
                'l' => {
                    style.bold = true;
                }
                'o' => {
                    style.italic = true;
                }
                'n' => {
                    style.underline = true;
                }
                'm' => {
                    style.strikethrough = true;
                }
                _ => {
                    output.push(ch);
                    output.push(chars[index + 1].1);
                    run_start = current_len;
                    index += 2;
                    continue;
                }
            }

            run_start = output.len();
            index += 2;
            continue;
        }

        if glyph_code_point(ch).is_some() {
            has_glyphs = true;
        }
        output.push(ch);
        index += 1;
    }

    if output.len() > run_start {
        runs.push(MinecraftTextRunSpec {
            start: run_start,
            end: output.len(),
            color: current_color,
            style,
        });
    }

    if runs.is_empty() && !output.is_empty() {
        runs.push(MinecraftTextRunSpec {
            start: 0,
            end: output.len(),
            color: current_color,
            style,
        });
    }

    ParsedMinecraftText {
        text: output,
        runs,
        has_glyphs,
        has_obfuscated,
    }
}

fn build_text_runs(runs: &[MinecraftTextRunSpec]) -> Vec<TextRun> {
    runs.iter()
        .map(|spec| {
            make_text_run(
                spec.end.saturating_sub(spec.start),
                InlinePieceStyle {
                    color: spec.color,
                    style: spec.style,
                },
            )
        })
        .collect()
}

fn make_text_run(len: usize, piece_style: InlinePieceStyle) -> TextRun {
    TextRun {
        len,
        font: Font {
            family: "HarmonyOS Sans".into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: if piece_style.style.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if piece_style.style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        },
        color: piece_style.color,
        background_color: None,
        background_corner_radius: None,
        background_padding: None,
        underline: piece_style.style.underline.then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(piece_style.color),
            wavy: false,
        }),
        strikethrough: piece_style
            .style
            .strikethrough
            .then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(piece_style.color),
            }),
    }
}

fn obfuscated_frame_tick() -> u64 {
    static START: Lazy<Instant> = Lazy::new(Instant::now);
    START.elapsed().as_millis() as u64 / OBFUSCATED_FRAME.as_millis() as u64
}

fn apply_obfuscated_frame(parsed: &ParsedMinecraftText, frame: u64) -> ParsedMinecraftText {
    if !parsed.has_obfuscated {
        return parsed.clone();
    }

    let mut text = String::with_capacity(parsed.text.len());
    for run in &parsed.runs {
        let range_text = &parsed.text[run.start..run.end];
        if run.style.obfuscated {
            for (index, ch) in range_text.chars().enumerate() {
                text.push(obfuscate_char(ch, frame, run.start + index));
            }
        } else {
            text.push_str(range_text);
        }
    }

    ParsedMinecraftText {
        text,
        runs: parsed.runs.clone(),
        has_glyphs: parsed.has_glyphs,
        has_obfuscated: parsed.has_obfuscated,
    }
}

fn obfuscate_char(ch: char, frame: u64, index: usize) -> char {
    if ch.is_whitespace() || !ch.is_ascii() || glyph_code_point(ch).is_some() {
        return ch;
    }

    const OBFUSCATED_ASCII: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let seed = frame
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(index as u64 * 0x85EB_CA6B)
        .wrapping_add(ch as u64);
    OBFUSCATED_ASCII[(seed as usize) % OBFUSCATED_ASCII.len()] as char
}

fn minecraft_format_color(code: char, default_color: Hsla) -> Option<Hsla> {
    let color_rgb = match code {
        '0' => 0x000000,
        '1' => 0x0000AA,
        '2' => 0x00AA00,
        '3' => 0x00AAAA,
        '4' => 0xAA0000,
        '5' => 0xAA00AA,
        '6' => 0xFFAA00,
        '7' => 0xAAAAAA,
        '8' => 0x555555,
        '9' => 0x5555FF,
        'a' => 0x55FF55,
        'b' => 0x55FFFF,
        'c' => 0xFF5555,
        'd' => 0xFF55FF,
        'e' => 0xFFFF55,
        'f' => 0xFFFFFF,
        'g' => 0xDDD605,
        'h' => 0xE3D4D1,
        'i' => 0xCECACA,
        'j' => 0x443A3B,
        'm' => 0x971607,
        'n' => 0xB4684D,
        'p' => 0xDEB12D,
        'q' => 0x47A036,
        's' => 0x2CBAA8,
        't' => 0x21497B,
        'u' => 0x9A5CC6,
        'v' => 0xEB7114,
        _ => return None,
    };
    Some(readable_minecraft_color(
        rgb(color_rgb).into(),
        default_color,
    ))
}

fn readable_minecraft_color(color: Hsla, default_color: Hsla) -> Hsla {
    let color_rgba = Rgba::from(color);
    let default_rgba = Rgba::from(default_color);
    let background = if relative_luminance(default_rgba) < 0.5 {
        Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }
    } else {
        Rgba {
            r: 0.08,
            g: 0.09,
            b: 0.11,
            a: 1.0,
        }
    };

    if contrast_ratio(color_rgba, background) >= 3.2 {
        return color;
    }

    let mut low = 0.0;
    let mut high = 1.0;
    let mut best = default_rgba;

    for _ in 0..10 {
        let factor = (low + high) * 0.5;
        let blended = blend_rgba(color_rgba, default_rgba, factor);
        if contrast_ratio(blended, background) >= 3.2 {
            best = blended;
            high = factor;
        } else {
            low = factor;
        }
    }

    best.into()
}

fn blend_rgba(from: Rgba, to: Rgba, factor: f32) -> Rgba {
    let factor = factor.clamp(0.0, 1.0);
    let inverse = 1.0 - factor;
    Rgba {
        r: from.r * inverse + to.r * factor,
        g: from.g * inverse + to.g * factor,
        b: from.b * inverse + to.b * factor,
        a: from.a,
    }
}

fn contrast_ratio(foreground: Rgba, background: Rgba) -> f32 {
    let foreground_luminance = relative_luminance(foreground);
    let background_luminance = relative_luminance(background);
    let lighter = foreground_luminance.max(background_luminance);
    let darker = foreground_luminance.min(background_luminance);
    (lighter + 0.05) / (darker + 0.05)
}

fn relative_luminance(color: Rgba) -> f32 {
    fn channel(value: f32) -> f32 {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    let red = channel(color.r);
    let green = channel(color.g);
    let blue = channel(color.b);
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn obfuscated_frame_changes_ascii_text() {
        let parsed = parse_minecraft_formatted_text("§kabcdef", rgb(0xffffff).into());

        let first = apply_obfuscated_frame(&parsed, 1);
        let second = apply_obfuscated_frame(&parsed, 2);

        assert!(parsed.has_obfuscated);
        assert_eq!(first.text.len(), parsed.text.len());
        assert_eq!(second.text.len(), parsed.text.len());
        assert_ne!(first.text, second.text);
    }

    #[::core::prelude::v1::test]
    fn normal_formatted_text_uses_cacheable_plain_output() {
        let first = cached_minecraft_formatted_text("§aHello", rgb(0xffffff).into());
        let second = cached_minecraft_formatted_text("§aHello", rgb(0xffffff).into());

        assert_eq!(first.text, "Hello");
        assert_eq!(second.text, "Hello");
        assert_eq!(first.runs.len(), second.runs.len());
        assert!(!first.has_obfuscated);
    }
}
