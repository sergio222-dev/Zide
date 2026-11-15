//! The model and the effort of a chat, picked from the two menus under the input. A picked value
//! reaches a running process at once (`set_model`, `apply_flag_settings`); a chat without a
//! process starts with it. Nothing picked means the default, which the dropdown names.

use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::*;
use zide_claude::{Defaults, Event, Model};

use super::Chat;
use super::Chats;

/// What is offered before the CLI ever said which models it has: its aliases.
pub(super) fn fallback_models() -> Vec<Model> {
    [
        ("Fable", "fable"),
        ("Opus", "opus"),
        ("Sonnet", "sonnet"),
        ("Haiku", "haiku"),
    ]
    .map(|(name, value)| Model {
        value: value.to_owned(),
        name: name.to_owned(),
    })
    .to_vec()
}

/// What to call the model `value`: its name among `models`, else `value` itself.
fn name_of<'a>(value: &'a str, models: &'a [Model]) -> &'a str {
    models
        .iter()
        .find(|model| model.value == value)
        .map_or(value, |model| model.name.as_str())
}

pub(super) const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The first row: what is used when nothing is picked.
pub(super) fn default_row(default: Option<&str>) -> String {
    match default {
        Some(name) => format!("Default ({name})"),
        None => "Default".to_owned(),
    }
}

/// The model rows: the default, the `models` offered and, when the value is none of them,
/// that exact name (a conversation made with a model no longer offered).
pub(super) fn model_rows(
    defaults: &Defaults,
    models: &[Model],
    value: Option<&str>,
) -> Vec<String> {
    let mut rows = vec![default_row(defaults.model.as_deref())];
    rows.extend(models.iter().map(|model| model.name.clone()));
    if let Some(value) = value
        && !models.iter().any(|model| model.value == value)
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

/// What a model row stands for: `None` for the default, the model's value, or the exact name.
fn model_value(row: &str, models: &[Model], rows: &[String]) -> Option<String> {
    if rows.first().map(String::as_str) == Some(row) {
        return None;
    }
    Some(
        models
            .iter()
            .find(|model| model.name == row)
            .map_or(row, |model| model.value.as_str())
            .to_owned(),
    )
}

/// The row that shows `value`.
pub(super) fn model_row(value: Option<&str>, models: &[Model], rows: &[String]) -> String {
    match value {
        None => rows[0].clone(),
        Some(value) => name_of(value, models).to_owned(),
    }
}

fn effort_value(row: &str, rows: &[String]) -> Option<String> {
    (rows.first().map(String::as_str) != Some(row)).then(|| row.to_owned())
}

/// What the model button says: the name of the model in use.
pub(super) fn model_label(value: Option<&str>, defaults: &Defaults, models: &[Model]) -> String {
    match value.or(defaults.model.as_deref()) {
        Some(value) => name_of(value, models).to_owned(),
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
        cx: &mut Context<Chats>,
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
        cx: &mut Context<Chats>,
    ) -> Subscription {
        cx.subscribe_in(
            slider,
            window,
            move |this: &mut Chats, _, event: &SliderEvent, _, cx| {
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

impl Chats {
    /// Makes the effort slider of the active chat stand where its effort is, after that changed
    /// behind it (a stored conversation was loaded, the default was chosen again).
    pub(super) fn sync_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = self.defaults.clone();
        let chat = self.active_mut();
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
        let shown = self
            .find(id)
            .and_then(|chat| chat.effort.clone())
            .or_else(|| self.defaults.effort.clone());
        if shown.as_deref() == Some(effort) {
            return;
        }
        self.claude_pick_effort(id, effort, cx);
    }

    /// The row the user picked in the model menu.
    pub(super) fn claude_pick_model(&mut self, id: u64, row: &str, cx: &mut Context<Self>) {
        let current = self.find(id).and_then(|chat| chat.model.clone());
        let rows = model_rows(&self.defaults, &self.models, current.as_deref());
        let value = model_value(row, &self.models, &rows);
        let Some(chat) = self.find(id) else { return };
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
        let rows = effort_rows(&self.defaults);
        let value = effort_value(row, &rows);
        let Some(chat) = self.find(id) else { return };
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
        self.workers.clear();
        self.prune_worker_markdown();
    }
}

#[cfg(test)]
mod tests {
    use zide_claude::{Defaults, Model};

    use super::{
        effort_label, effort_position, effort_rows, effort_value, fallback_models, model_label,
        model_row, model_rows, model_value,
    };

    fn models() -> Vec<Model> {
        [
            ("opus", "Opus 5.5"),
            ("sonnet", "Sonnet 5.5"),
            ("claude-haiku-4-5", "Haiku 4.5"),
        ]
        .map(|(value, name)| Model {
            value: value.into(),
            name: name.into(),
        })
        .to_vec()
    }

    fn defaults() -> Defaults {
        Defaults {
            model: Some("sonnet".into()),
            effort: Some("medium".into()),
            ..Defaults::default()
        }
    }

    #[test]
    fn the_first_row_names_the_default() {
        assert_eq!(
            model_rows(&defaults(), &models(), None)[0],
            "Default (sonnet)"
        );
        assert_eq!(effort_rows(&defaults())[0], "Default (medium)");
        assert_eq!(
            model_rows(&Defaults::default(), &models(), None)[0],
            "Default"
        );
    }

    #[test]
    fn every_model_offered_gets_a_row() {
        let rows = model_rows(&defaults(), &models(), None);
        assert_eq!(
            rows,
            ["Default (sonnet)", "Opus 5.5", "Sonnet 5.5", "Haiku 4.5"]
        );
        let aliases = model_rows(&Defaults::default(), &fallback_models(), None);
        assert_eq!(aliases, ["Default", "Fable", "Opus", "Sonnet", "Haiku"]);
    }

    #[test]
    fn a_model_the_aliases_do_not_cover_gets_its_own_row() {
        let rows = model_rows(&defaults(), &models(), Some("claude-old-1"));
        assert_eq!(rows.last().map(String::as_str), Some("claude-old-1"));
        assert_eq!(model_rows(&defaults(), &models(), Some("opus")).len(), 4);
        assert_eq!(
            model_row(Some("claude-old-1"), &models(), &rows),
            "claude-old-1"
        );
        assert_eq!(
            model_value("claude-old-1", &models(), &rows).as_deref(),
            Some("claude-old-1")
        );
    }

    #[test]
    fn rows_and_values_map_both_ways() {
        let rows = model_rows(&defaults(), &models(), None);
        assert_eq!(model_value("Default (sonnet)", &models(), &rows), None);
        assert_eq!(
            model_value("Opus 5.5", &models(), &rows).as_deref(),
            Some("opus")
        );
        assert_eq!(
            model_value("Haiku 4.5", &models(), &rows).as_deref(),
            Some("claude-haiku-4-5")
        );
        assert_eq!(model_row(Some("opus"), &models(), &rows), "Opus 5.5");
        assert_eq!(model_row(None, &models(), &rows), "Default (sonnet)");
        let efforts = effort_rows(&defaults());
        assert_eq!(effort_value("Default (medium)", &efforts), None);
        assert_eq!(effort_value("high", &efforts).as_deref(), Some("high"));
    }

    #[test]
    fn the_buttons_name_what_is_in_use() {
        assert_eq!(model_label(None, &defaults(), &models()), "Sonnet 5.5");
        assert_eq!(
            model_label(Some("opus"), &defaults(), &models()),
            "Opus 5.5"
        );
        assert_eq!(
            model_label(Some("claude-old-1"), &defaults(), &models()),
            "claude-old-1"
        );
        assert_eq!(
            model_label(None, &Defaults::default(), &models()),
            "Default"
        );
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
