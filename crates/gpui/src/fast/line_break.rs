//! CJK break constraints from longbridge/gpui-fast fast/line_break.rs.
//! Upstream revision: 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
//! SPDX-License-Identifier: Apache-2.0
//! The upstream now uses ICU UAX #14; these retained upstream constraints
//! preserve the legacy wrapper's API and do not require a new dependency.

/// Whether a line may wrap between `prev` and `c`, two characters with no
/// space between them, in text that otherwise breaks between any two
/// characters.
pub(crate) fn allows_break_between(prev: char, c: char) -> bool {
    !never_starts_line(c)
        && !never_ends_line(prev)
        && !(prev == '—' && c == '—')
        && !(is_cjk(prev) && closes_cjk_text(c))
}

/// ASCII punctuation that, written right after CJK text, closes it like its
/// full-width form would (UAX #14 classes CL, CP, EX, IS and PO): `你好,世界`.
/// After Latin text a break before it stays, since that is what lets a long
/// path or URL wrap.
fn closes_cjk_text(c: char) -> bool {
    matches!(c, ',' | '.' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '%')
}

/// Han, kana, Hangul and full-width forms, the scripts that wrap between
/// characters rather than at spaces.
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{3000}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF00}'..='\u{FFEF}'
            | '\u{20000}'..='\u{3FFFF}'
    )
}

/// Closing and full-width punctuation that must stay at the end of the line
/// before it (clreq's 行首禁则).
fn never_starts_line(c: char) -> bool {
    matches!(
        c,
        // Full-width stops, commas and marks.
        '、' | '。' | '，' | '．' | '：' | '；' | '？' | '！' | '｡' | '､'
            // Closing brackets and quotes.
            | '）' | '］' | '｝' | '」' | '』' | '】' | '〉' | '》' | '〕' | '〗' | '〙'
            | '〛' | '〞' | '〟' | '｠' | '｣' | '’' | '”'
            // Ellipses, middle dots and the like, iteration marks.
            | '…' | '‥' | '・' | '･' | '〜' | '‼' | '⁇' | '⁈' | '⁉' | '々' | '〻' | 'ゝ'
            | 'ゞ' | 'ヽ' | 'ヾ'
            // Postfix full-width signs.
            | '％'
    )
}

/// Opening brackets and quotes that must start the line after them
/// (clreq's 行尾禁则).
///
/// The ASCII brackets are here too, as in UAX #14 (class OP). This only takes
/// breaks away, and only after the bracket: the wrappers break before a word
/// character only after a space, so in Latin text, code and URLs, where `(`
/// is followed by a letter, nothing changes, while `(` no longer ends a line
/// before the CJK text it opens. ASCII closing punctuation is left out, since
/// a break before `)` or `?` is what lets a long path or URL wrap at all.
fn never_ends_line(c: char) -> bool {
    matches!(
        c,
        // ASCII brackets.
        '(' | '[' | '{'
            // Full-width and CJK brackets and quotes.
            | '（'
            | '［'
            | '｛'
            | '「'
            | '『'
            | '【'
            | '〈'
            | '《'
            | '〔'
            | '〖'
            | '〘'
            | '〚'
            | '〝'
            | '｟'
            | '｢'
            | '‘'
            | '“'
    )
}

