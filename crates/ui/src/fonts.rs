//! The font families Zide draws in: one monospace family for code (the editor, the merge editor
//! and the chat's code) and one for the interface. Chosen in the Fonts dialog and kept in the
//! settings file; a choice that is not installed falls back to Zide's own pick.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::WindowExt;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::*;

use crate::settings::{self, Fonts};
use crate::{Workspace, theme};

/// Monospace families, in order of preference. GPUI does not resolve the generic `monospace`.
const EDITOR_FONTS: &[&str] = &[
    "MonoLisaCode",
    "MonoLisa",
    "JetBrains Mono",
    "Iosevka Nerd Font Mono",
    "0xProto Nerd Font Mono",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Adwaita Mono",
    "Liberation Mono",
    "Menlo",
    "Consolas",
];

/// The interface family when none is chosen. The bundled default (`.SystemUIFont`) showed no
/// bold; naming the family finds its bold face.
const UI_FONT: &str = "Noto Sans";

/// The first row of both lists: leave the choice to Zide.
const DEFAULT_ITEM: &str = "Default";

static EDITOR: RwLock<Option<SharedString>> = RwLock::new(None);
/// Bumped each time the fonts change, so what measured the old one measures again.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The family code is drawn in, as last applied.
pub fn editor() -> SharedString {
    EDITOR
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| EDITOR_FONTS[0].into())
}

/// Changes whenever `apply` runs.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn is_installed(family: &str, installed: &[String]) -> bool {
    installed.iter().any(|name| name == family)
}

/// The chosen family when it is installed, else the first installed of `EDITOR_FONTS`.
fn pick_editor(chosen: Option<&str>, installed: &[String]) -> SharedString {
    chosen
        .into_iter()
        .chain(EDITOR_FONTS.iter().copied())
        .find(|family| is_installed(family, installed))
        .unwrap_or(EDITOR_FONTS[0])
        .to_string()
        .into()
}

fn pick_ui(chosen: Option<&str>, installed: &[String]) -> SharedString {
    chosen
        .filter(|family| is_installed(family, installed))
        .unwrap_or(UI_FONT)
        .to_string()
        .into()
}

/// Draws code and the interface in the fonts the settings name, in every window.
pub fn apply(cx: &mut App) {
    let chosen = settings::fonts();
    let installed = cx.text_system().all_font_names();
    *EDITOR.write().unwrap_or_else(|e| e.into_inner()) =
        Some(pick_editor(chosen.editor.as_deref(), &installed));
    let ui = pick_ui(chosen.ui.as_deref(), &installed);
    gpui_kit::component::Theme::update(cx, |theme| theme.font_family = ui);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    cx.refresh_windows();
}

/// Installed families, sorted and without repeats.
fn installed(cx: &App) -> Vec<String> {
    let mut names = cx.text_system().all_font_names();
    names.retain(|name| !name.starts_with('.')); // GPUI's own aliases
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// Whether every character of `family` is as wide as the others; GPUI does not say, so a narrow
/// and a wide letter are compared.
fn is_monospace(family: &str, cx: &App) -> bool {
    let text_system = cx.text_system();
    let id = text_system.resolve_font(&font(SharedString::from(family.to_string())));
    let width = |c| {
        text_system
            .advance(id, px(16.), c)
            .ok()
            .map(|advance| f32::from(advance.width))
    };
    matches!((width('i'), width('W')), (Some(i), Some(w)) if i > 0. && (i - w).abs() < 0.01)
}

type FontSelect = Entity<SelectState<Vec<SharedString>>>;

/// `families` after the default row, with `chosen` selected (and listed, even when the filter
/// left it out).
fn select(
    mut families: Vec<String>,
    chosen: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> FontSelect {
    if let Some(chosen) = chosen
        && !families.iter().any(|family| family == chosen)
    {
        families.insert(0, chosen.to_string());
    }
    let items: Vec<SharedString> = std::iter::once(DEFAULT_ITEM.to_string())
        .chain(families)
        .map(SharedString::from)
        .collect();
    let selected = chosen
        .and_then(|chosen| items.iter().position(|item| item == chosen))
        .unwrap_or(0);
    cx.new(|cx| {
        SelectState::new(
            items,
            Some(gpui_kit::component::IndexPath::new(selected)),
            window,
            cx,
        )
        .searchable(true)
    })
}

fn chosen(item: &Option<SharedString>) -> Option<String> {
    item.as_ref()
        .filter(|item| item.as_ref() != DEFAULT_ITEM)
        .map(ToString::to_string)
}

/// The Fonts dialog: a list of monospace families for code and one of every family for the
/// interface. A choice takes effect at once and is saved.
pub struct FontsForm {
    editor: FontSelect,
    ui: FontSelect,
    error: Option<String>,
}

impl FontsForm {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = settings::fonts();
        let all = installed(cx);
        let monospace = all
            .iter()
            .filter(|family| is_monospace(family, cx))
            .cloned()
            .collect();
        let editor = select(monospace, current.editor.as_deref(), window, cx);
        let ui = select(all, current.ui.as_deref(), window, cx);
        cx.subscribe_in(&editor, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(item) = event;
            let fonts = Fonts {
                editor: chosen(item),
                ..settings::fonts()
            };
            this.save(fonts, cx);
        })
        .detach();
        cx.subscribe_in(&ui, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(item) = event;
            let fonts = Fonts {
                ui: chosen(item),
                ..settings::fonts()
            };
            this.save(fonts, cx);
        })
        .detach();
        Self {
            editor,
            ui,
            error: None,
        }
    }

    fn save(&mut self, fonts: Fonts, cx: &mut Context<Self>) {
        self.error = settings::set_fonts(fonts).err();
        apply(cx);
        cx.notify();
    }
}

fn row(label: &'static str, hint: &'static str, select: &FontSelect) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().text_color(rgb(theme::STRONG)).child(label))
                .child(div().text_color(rgb(theme::MUTED)).child(hint)),
        )
        .child(Select::new(select).menu_max_h(px(320.)))
}

impl Render for FontsForm {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(row(
                "Code",
                "the editor, the merge editor and the chat's code",
                &self.editor,
            ))
            .child(row("Interface", "menus, panels and dialogs", &self.ui))
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_color(rgb(theme::ERROR)).child(error)),
            )
    }
}

impl Workspace {
    /// The Fonts command: chooses the families code and the interface are drawn in.
    pub(crate) fn configure_fonts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| FontsForm::new(window, cx));
        window.open_dialog(cx, move |dialog, window, _| {
            let width = px(520.).min(window.viewport_size().width * 0.92);
            dialog.title("Fonts").width(width).child(form.clone())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[::core::prelude::v1::test]
    fn an_installed_choice_wins_and_a_missing_one_falls_back() {
        let installed = names(&["Fira Code", "JetBrains Mono", "Inter"]);
        assert_eq!(pick_editor(Some("Fira Code"), &installed), "Fira Code");
        assert_eq!(pick_editor(Some("Gone"), &installed), "JetBrains Mono");
        assert_eq!(pick_editor(None, &installed), "JetBrains Mono");
        assert_eq!(pick_editor(None, &[]), EDITOR_FONTS[0]);
        assert_eq!(pick_ui(Some("Inter"), &installed), "Inter");
        assert_eq!(pick_ui(Some("Gone"), &installed), UI_FONT);
        assert_eq!(pick_ui(None, &installed), UI_FONT);
    }

    #[::core::prelude::v1::test]
    fn the_default_row_leaves_the_choice_to_zide() {
        assert_eq!(chosen(&Some(DEFAULT_ITEM.into())), None);
        assert_eq!(chosen(&None), None);
        assert_eq!(chosen(&Some("Inter".into())), Some("Inter".into()));
    }
}
