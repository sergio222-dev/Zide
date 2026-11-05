//! Word-boundary rules, ported from IntelliJ's `NameUtilCore`.
//!
//! Character classes use the Rust standard library. `is_digit` also accepts
//! non-ASCII numerics that Java would classify differently; ASCII is exact.

pub(crate) fn is_digit(c: char) -> bool {
    c.is_numeric()
}

pub(crate) fn is_letter(c: char) -> bool {
    c.is_alphabetic()
}

pub(crate) fn is_letter_or_digit(c: char) -> bool {
    is_letter(c) || is_digit(c)
}

/// Single-char lowercase mapping; keeps `c` when the mapping is not one char.
pub(crate) fn to_lower(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Single-char uppercase mapping; keeps `c` when the mapping is not one char.
pub(crate) fn to_upper(c: char) -> char {
    let mut it = c.to_uppercase();
    match (it.next(), it.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

/// Case-insensitive char equality, like Kotlin's `Char.equals(other, ignoreCase = true)`.
pub(crate) fn eq_ignore_case(a: char, b: char) -> bool {
    a == b || to_upper(a) == to_upper(b) || to_lower(a) == to_lower(b)
}

/// Returns the end of the word that starts at `start` (may be `text.len()`).
/// `start` must be `< text.len()`.
pub fn next_word(text: &[char], start: usize) -> usize {
    if !is_letter_or_digit(text[start]) {
        return start + 1;
    }

    let mut i = start;
    while i < text.len() && is_digit(text[i]) {
        i += 1;
    }
    if i > start {
        // digits form a separate hump
        return i;
    }

    while i < text.len() && text[i].is_uppercase() {
        i += 1;
    }
    if i > start + 1 {
        // several consecutive uppercase letters form a hump
        if i == text.len() || !is_letter(text[i]) {
            return i;
        }
        return i - 1;
    }

    if i == start {
        i += 1;
    }
    while i < text.len() && is_letter(text[i]) && !is_word_start(text, i) {
        i += 1;
    }
    i
}

/// Tells whether a word (camel hump, digit run, ...) starts at index `i`.
pub fn is_word_start(text: &[char], i: usize) -> bool {
    let cur = text[i];
    let prev = if i > 0 { Some(text[i - 1]) } else { None };
    if cur.is_uppercase() {
        if prev.is_some_and(|p| p.is_uppercase()) {
            // not in the middle of an all-caps word
            return text.get(i + 1).is_some_and(|n| n.is_lowercase());
        }
        return true;
    }
    if is_digit(cur) {
        return true;
    }
    if !is_letter(cur) {
        return false;
    }
    if is_ideographic(cur) {
        // every ideograph is a separate word
        return true;
    }
    i == 0 || !is_letter_or_digit(text[i - 1]) || prev.is_some_and(|p| is_kana_break(cur, p))
}

fn is_ideographic(c: char) -> bool {
    matches!(c as u32,
        0x3006 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0x20000..=0x3134F)
}

#[derive(PartialEq, Eq)]
enum Script {
    Hiragana,
    Katakana,
    Common,
    Other,
}

fn script(c: char) -> Script {
    match c as u32 {
        0x3041..=0x3096 | 0x309D..=0x309F => Script::Hiragana,
        0x30A1..=0x30FA | 0x30FD..=0x30FF | 0xFF66..=0xFF6F | 0xFF71..=0xFF9D => Script::Katakana,
        0x30FC => Script::Common,
        _ if !c.is_alphabetic() => Script::Common,
        _ => Script::Other,
    }
}

fn maybe_kana(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x3358 | 0xFF66..=0xFF9D)
}

fn is_kana_break(cur: char, prev: char) -> bool {
    if !maybe_kana(cur) && !maybe_kana(prev) {
        return false;
    }
    let (cur_script, prev_script) = (script(cur), script(prev));
    if cur_script == prev_script {
        return false;
    }
    let involves_kana = matches!(cur_script, Script::Katakana | Script::Hiragana)
        || matches!(prev_script, Script::Katakana | Script::Hiragana);
    involves_kana && prev_script != Script::Common && cur_script != Script::Common
}
