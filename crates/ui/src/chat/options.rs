//! The model and the effort of a chat, picked from the two menus under the input. A picked value
//! reaches a running process at once (`set_model`, `apply_flag_settings`); a chat without a
//! process starts with it. Nothing picked means the default, which the dropdown names.

use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::*;
use zide_claude::{Defaults, Event};

use super::Chat;
use crate::Workspace;

/// The models offered, as the dropdown names them and as `claude --model` takes them.
const MODELS: [(&str, &str); 4] = [
    ("Fable", "fable"),
    ("Opus", "opus"),
    ("Sonnet", "sonnet"),
    ("Haiku", "haiku"),
];

pub(super) const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The first row: what is used when nothing is picked.
pub(super) fn default_row(default: Option<&str>) -> String {
    match default {
        Some(name) => format!("Default ({name})"),
        None => "Default".to_owned(),
    }
}

/// The model rows: the default, the aliases and, when the value is neither, that exact name
/// (a conversation made with a model the aliases no longer point to).
pub(super) fn model_rows(defaults: &Defaults, value: Option<&str>) -> Vec<String> {
    let mut rows = vec![default_row(defaults.model.as_deref())];
    rows.extend(MODELS.iter().map(|(label, _)| (*label).to_owned()));
    if let Some(value) = value
        && !MODELS.iter().any(|(_, alias)| *alias == value)
    {
        rows.push(value.to_owned());
    }
    rows
}

pub(super) fn effort_rows(defaults: &Defaults) -> Vec<String> {
    let mut rows = vec![default_row(defaults.effort.as_deref())];
    rows.extend(EFFORTS.iter().map(|effort| (*effort).to_owned()));
    rows
}

/// What a model row stands for: `None` for the default, the alias, or the exact name.
fn model_value(row: &str, rows: &[String]) -> Option<String> {
    if rows.first().map(String::as_str) == Some(row) {
        return None;
    }
    Some(
        MODELS
            .iter()
            .find(|(label, _)| *label == row)
            .map_or(row, |(_, alias)| alias)
            .to_owned(),
    )
}

/// The row that shows `value`.
pub(super) fn model_row(value: Option<&str>, rows: &[String]) -> String {
    match value {
        None => rows[0].clone(),
        Some(value) => MODELS
            .iter()
            .find(|(_, alias)| *alias == value)
            .map_or(value, |(label, _)| label)
            .to_owned(),
    }
}

fn effort_value(row: &str, rows: &[String]) -> Option<String> {
    (rows.first().map(String::as_str) != Some(row)).then(|| row.to_owned())
}

/// What the model button says: the name of the model in use.
pub(super) fn model_label(value: Option<&str>, defaults: &Defaults) -> String {
    match value.or(defaults.model.as_deref()) {
        Some(alias) => MODELS
            .iter()
            .find(|(_, a)| *a == alias)
            .map_or(alias, |(label, _)| label)
            .to_owned(),
        None => "Default".to_owned(),
    }
}

/// The effort in use, as a word with a capital: "Medium".
pub(super) fn effort_label(value: Option<&str>, defaults: &Defaults) -> String {
    let word = value.or(defaults.effort.as_deref()).unwrap_or("default");
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Where the slider stands for an effort: its position among `EFFORTS`.
pub(super) fn effort_position(value: Option<&str>, defaults: &Defaults) -> f32 {
    let word = value.or(defaults.effort.as_deref());
    EFFORTS
        .iter()
        .position(|effort| Some(*effort) == word)
        .unwrap_or(1) as f32
}

impl Chat {
    /// The slider that picks the effort: one step per level.
    pub(super) fn new_effort_slider(
        defaults: &Defaults,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<SliderState> {
        let _ = window;
        cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max((EFFORTS.len() - 1) as f32)
                .step(1.)
                .default_value(effort_position(None, defaults))
        })
    }

    /// Tells the workspace what was picked on the slider.
    pub(super) fn effort_events(
        id: u64,
        slider: &Entity<SliderState>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Subscription {
        cx.subscribe_in(
            slider,
            window,
            move |this: &mut Workspace, _, event: &SliderEvent, _, cx| {
                let SliderEvent::Change(value) = event else {
                    return;
                };
                let at = value.end().round().max(0.) as usize;
                if let Some(effort) = EFFORTS.get(at) {
                    this.claude_slide_effort(id, effort, cx);
                }
            },
        )
    }
}

impl Workspace {
    /// Makes the effort slider of the active chat stand where its effort is, after that changed
    /// behind it (a stored conversation was loaded, the default was chosen again).
    pub(super) fn sync_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let defaults = chats.defaults.clone();
        let chat = chats.active_mut();
        if !chat.options_stale {
            return;
        }
        chat.options_stale = false;
        let at = effort_position(chat.effort.as_deref(), &defaults);
        chat.effort_slider
            .update(cx, |state, cx| state.set_value(at, window, cx));
    }

    /// The slider moved to `effort`. Landing where the chat already is (the slider was set to
    /// show it) changes nothing.
    fn claude_slide_effort(&mut self, id: u64, effort: &str, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let shown = chats
            .find(id)
            .and_then(|chat| chat.effort.clone())
            .or_else(|| chats.defaults.effort.clone());
        if shown.as_deref() == Some(effort) {
            return;
        }
        self.claude_pick_effort(id, effort, cx);
    }

    /// The row the user picked in the model menu.
    pub(super) fn claude_pick_model(&mut self, id: u64, row: &str, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let current = chats.find(id).and_then(|chat| chat.model.clone());
        let rows = model_rows(&chats.defaults, current.as_deref());
        let value = model_value(row, &rows);
        let Some(chat) = chats.find(id) else { return };
        if chat.model == value {
            return;
        }
        chat.model = value.clone();
        match (chat.session.as_mut(), value) {
            (Some(session), Some(model)) => {
                let _ = session.set_model(&model);
            }
            // back to the default: only a new process knows what that is
            (Some(_), None) => chat.stop(),
            (None, _) => {}
        }
        cx.notify();
    }

    /// The row the user picked for the effort.
    pub(super) fn claude_pick_effort(&mut self, id: u64, row: &str, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let rows = effort_rows(&chats.defaults);
        let value = effort_value(row, &rows);
        let Some(chat) = chats.find(id) else { return };
        if chat.effort == value {
            return;
        }
        chat.effort = value.clone();
        chat.options_stale = true; // the slider follows
        match (chat.session.as_mut(), value) {
            (Some(session), Some(effort)) => {
                let _ = session.set_effort(&effort);
            }
            (Some(_), None) => chat.stop(),
            (None, _) => {}
        }
        cx.notify();
    }
}

impl Chat {
    /// Stops the process; the next message starts a new one with the settings now chosen.
    fn stop(&mut self) {
        self.run += 1;
        self.session = None;
        self.transcript.apply(Event::Exit);
    }
}

#[cfg(test)]
mod tests {
    use zide_claude::Defaults;

    use super::{
        effort_label, effort_position, effort_rows, effort_value, model_label, model_row,
        model_rows, model_value,
    };

    fn defaults() -> Defaults {
        Defaults {
            model: Some("sonnet".into()),
            effort: Some("medium".into()),
            ..Defaults::default()
        }
    }

    #[test]
    fn the_first_row_names_the_default() {
        assert_eq!(model_rows(&defaults(), None)[0], "Default (sonnet)");
        assert_eq!(effort_rows(&defaults())[0], "Default (medium)");
        assert_eq!(model_rows(&Defaults::default(), None)[0], "Default");
    }

    #[test]
    fn a_model_the_aliases_do_not_cover_gets_its_own_row() {
        let rows = model_rows(&defaults(), Some("claude-old-1"));
        assert_eq!(rows.last().map(String::as_str), Some("claude-old-1"));
        assert_eq!(model_rows(&defaults(), Some("opus")).len(), 5);
        assert_eq!(model_row(Some("claude-old-1"), &rows), "claude-old-1");
        assert_eq!(
            model_value("claude-old-1", &rows).as_deref(),
            Some("claude-old-1")
        );
    }

    #[test]
    fn rows_and_values_map_both_ways() {
        let rows = model_rows(&defaults(), None);
        assert_eq!(model_value("Default (sonnet)", &rows), None);
        assert_eq!(model_value("Opus", &rows).as_deref(), Some("opus"));
        assert_eq!(model_row(Some("opus"), &rows), "Opus");
        assert_eq!(model_row(None, &rows), "Default (sonnet)");
        let efforts = effort_rows(&defaults());
        assert_eq!(effort_value("Default (medium)", &efforts), None);
        assert_eq!(effort_value("high", &efforts).as_deref(), Some("high"));
    }

    #[test]
    fn the_buttons_name_what_is_in_use() {
        assert_eq!(model_label(None, &defaults()), "Sonnet");
        assert_eq!(model_label(Some("opus"), &defaults()), "Opus");
        assert_eq!(model_label(None, &Defaults::default()), "Default");
        assert_eq!(effort_label(None, &defaults()), "Medium");
        assert_eq!(effort_label(Some("xhigh"), &defaults()), "Xhigh");
    }

    #[test]
    fn the_slider_stands_on_the_effort_in_use() {
        assert_eq!(effort_position(None, &defaults()), 1.0);
        assert_eq!(effort_position(Some("max"), &defaults()), 4.0);
        assert_eq!(effort_position(None, &Defaults::default()), 1.0);
    }
}
