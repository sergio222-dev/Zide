//! The model and the effort of a chat, picked from two dropdowns over the header. A picked value
//! reaches a running process at once (`set_model`, `apply_flag_settings`); a chat without a
//! process starts with it. Nothing picked means the default, which the dropdown names.

use gpui_kit::component::IndexPath;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
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

const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub(super) type Dropdown = Entity<SelectState<Vec<String>>>;

/// The first row: what is used when nothing is picked.
fn default_row(default: Option<&str>) -> String {
    match default {
        Some(name) => format!("Default ({name})"),
        None => "Default".to_owned(),
    }
}

/// The model rows: the default, the aliases and, when the value is neither, that exact name
/// (a conversation made with a model the aliases no longer point to).
fn model_rows(defaults: &Defaults, value: Option<&str>) -> Vec<String> {
    let mut rows = vec![default_row(defaults.model.as_deref())];
    rows.extend(MODELS.iter().map(|(label, _)| (*label).to_owned()));
    if let Some(value) = value
        && !MODELS.iter().any(|(_, alias)| *alias == value)
    {
        rows.push(value.to_owned());
    }
    rows
}

fn effort_rows(defaults: &Defaults) -> Vec<String> {
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
fn model_row(value: Option<&str>, rows: &[String]) -> String {
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

fn effort_row(value: Option<&str>, rows: &[String]) -> String {
    value.map_or_else(|| rows[0].clone(), str::to_owned)
}

impl Chat {
    pub(super) fn new_dropdowns(
        defaults: &Defaults,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> (Dropdown, Dropdown) {
        let model = cx.new(|cx| {
            SelectState::new(
                model_rows(defaults, None),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let effort = cx
            .new(|cx| SelectState::new(effort_rows(defaults), Some(IndexPath::new(0)), window, cx));
        (model, effort)
    }
}

impl Workspace {
    /// Makes the dropdowns of the active chat show its model and effort, after they changed
    /// behind them (a stored conversation was loaded).
    pub(super) fn sync_dropdowns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &mut self.claude else {
            return;
        };
        let defaults = chats.defaults.clone();
        let chat = chats.active_mut();
        if !chat.dropdowns_stale {
            return;
        }
        chat.dropdowns_stale = false;
        let rows = model_rows(&defaults, chat.model.as_deref());
        let (model, effort) = (
            model_row(chat.model.as_deref(), &rows),
            effort_row(chat.effort.as_deref(), &effort_rows(&defaults)),
        );
        chat.model_select.update(cx, |state, cx| {
            state.set_items(rows, window, cx);
            state.set_selected_value(&model, window, cx);
        });
        chat.effort_select.update(cx, |state, cx| {
            state.set_selected_value(&effort, window, cx)
        });
    }

    /// The row the user picked in a model dropdown.
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

    /// The row the user picked in an effort dropdown.
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
        match (chat.session.as_mut(), value) {
            (Some(session), Some(effort)) => {
                let _ = session.set_effort(&effort);
            }
            (Some(_), None) => chat.stop(),
            (None, _) => {}
        }
        cx.notify();
    }

    /// The two dropdowns, for the header.
    pub(super) fn dropdowns_view(&self) -> Option<Div> {
        let chat = self.claude.as_ref()?.active();
        Some(
            div()
                .flex()
                .gap_2()
                .child(
                    Select::new(&chat.model_select)
                        .id("claude-model")
                        .title_prefix("Model: ")
                        .menu_width(px(220.))
                        .w(px(220.)),
                )
                .child(
                    Select::new(&chat.effort_select)
                        .id("claude-effort")
                        .title_prefix("Effort: ")
                        .menu_width(px(180.))
                        .w(px(180.)),
                ),
        )
    }
}

impl Chat {
    /// Stops the process; the next message starts a new one with the settings now chosen.
    fn stop(&mut self) {
        self.run += 1;
        self.session = None;
        self.transcript.apply(Event::Exit);
    }

    /// Subscriptions that tell the workspace what was picked.
    pub(super) fn dropdown_events(
        id: u64,
        model: &Dropdown,
        effort: &Dropdown,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Vec<Subscription> {
        vec![
            cx.subscribe_in(
                model,
                window,
                move |this: &mut Workspace, _, event: &SelectEvent<Vec<String>>, _, cx| {
                    let SelectEvent::Confirm(Some(row)) = event else {
                        return;
                    };
                    this.claude_pick_model(id, row, cx);
                },
            ),
            cx.subscribe_in(
                effort,
                window,
                move |this: &mut Workspace, _, event: &SelectEvent<Vec<String>>, _, cx| {
                    let SelectEvent::Confirm(Some(row)) = event else {
                        return;
                    };
                    this.claude_pick_effort(id, row, cx);
                },
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use zide_claude::Defaults;

    use super::{effort_row, effort_rows, effort_value, model_row, model_rows, model_value};

    fn defaults() -> Defaults {
        Defaults {
            model: Some("sonnet".into()),
            effort: Some("medium".into()),
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
        assert_eq!(effort_row(Some("high"), &efforts), "high");
        assert_eq!(effort_row(None, &efforts), "Default (medium)");
    }
}
