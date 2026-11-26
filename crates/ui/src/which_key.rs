//! The key hints panel, as LazyVim's which-key: while a Vim command is half typed (`d`, `g`,
//! `ci`, `z`), a small panel lists the keys that can finish it. What it lists is not written
//! here: it is the group being typed in `zide_vim`'s keymap, the same tree that runs the keys.

use std::time::Duration;

use gpui_kit::*;
use zide_vim::{Binding, Key, label};

use crate::{fonts, theme};

/// How long a half-typed command waits before the panel shows: a command typed in one go
/// (`dw`, `gg`) never flashes it.
pub const DELAY: Duration = Duration::from_millis(300);

/// Where a key goes in the panel, as which-key lays it out: digits, then letters (each
/// lowercase before its uppercase), then symbols, then Ctrl and named keys; groups after
/// every command.
fn order(binding: &Binding) -> (bool, u8, char, bool) {
    let (class, c) = match binding.key {
        Key::Char(c) if c.is_ascii_digit() => (0, c),
        Key::Char(c) if c.is_alphabetic() => (1, c.to_ascii_lowercase()),
        Key::Char(c) => (2, c),
        Key::Ctrl(c) => (3, c),
        _ => (4, ' '),
    };
    let upper = matches!(binding.key, Key::Char(c) if c.is_uppercase());
    (binding.is_group(), class, c, upper)
}

/// The panel, LazyVim style: the group's name, a key → description per row (groups, which
/// open more keys, last and marked with `+`) and how to close it.
pub fn render(group: &Binding, next: &[&Binding]) -> Div {
    let mut next = next.to_vec();
    next.sort_by_key(|binding| order(binding));
    let rows = next.into_iter().map(|binding| {
        let (color, description) = if binding.is_group() {
            (theme::CARET_BAR, format!("+{}", binding.description))
        } else {
            (theme::SYMBOL, binding.description.to_string())
        };
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .w(px(36.))
                    .flex()
                    .justify_end()
                    .text_color(rgb(theme::ICON_FOLDER))
                    .child(label(binding.key)),
            )
            .child(div().text_color(rgb(theme::DIM)).child("→"))
            .child(div().text_color(rgb(color)).child(description))
    });
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(theme::ACTIVE_BORDER))
        .bg(rgb(theme::POPUP))
        .shadow_lg()
        .font_family(fonts::editor())
        .text_sm()
        .child(
            div()
                .pb_1()
                .text_color(rgb(theme::CARET_BAR))
                .child(format!("+{}", group.description)),
        )
        .children(rows)
        .child(
            div()
                .pt_1()
                .flex()
                .justify_center()
                .gap_2()
                .text_xs()
                .child(div().text_color(rgb(theme::ICON_FOLDER)).child("esc"))
                .child(div().text_color(rgb(theme::DIM)).child("close")),
        )
}
