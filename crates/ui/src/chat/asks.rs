//! Claude's multiple-choice questions (`AskUserQuestion`), drawn with gpui-kit's questionnaire:
//! one question at a time, options with their descriptions, a field for something else, and
//! the answers going back to Claude when the last one is submitted.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::questionnaire::{
    Questionnaire, QuestionnaireActions, QuestionnaireChoice, QuestionnaireChoiceDefinition,
    QuestionnaireChoiceDescription, QuestionnaireChoices, QuestionnaireDescription,
    QuestionnaireError, QuestionnaireEvent, QuestionnaireInput, QuestionnaireInputDefinition,
    QuestionnaireItem, QuestionnaireItemDefinition, QuestionnaireNext, QuestionnairePrevious,
    QuestionnaireProgress, QuestionnaireSkip, QuestionnaireState, QuestionnaireSubmission,
    QuestionnaireSubmit, QuestionnaireTitle,
};
use gpui_kit::*;
use zide_claude::{Ask, Block, Question, QuestionOption, Resolution};

use super::Chat;
use crate::{Workspace, theme};

/// What a question needs on screen while it waits for the answer.
pub(super) struct AskState {
    state: Entity<QuestionnaireState>,
    _subscription: Subscription,
}

pub(super) type Asks = HashMap<String, AskState>;

/// The options the questionnaire can take: labels must be unique and not empty.
fn options(question: &Question) -> Vec<&QuestionOption> {
    let mut seen = Vec::new();
    question
        .options
        .iter()
        .filter(|option| {
            let new = !option.label.is_empty() && !seen.contains(&&option.label);
            seen.push(&option.label);
            new
        })
        .collect()
}

/// The name of the question at `index` inside its questionnaire.
fn name(index: usize) -> String {
    format!("q{index}")
}

/// The answer Claude gets for a question: the chosen labels, then what was typed instead.
fn joined(choices: &[String], typed: Option<&str>) -> Option<String> {
    let typed = typed.map(str::trim).filter(|text| !text.is_empty());
    let all: Vec<&str> = choices.iter().map(String::as_str).chain(typed).collect();
    (!all.is_empty()).then(|| all.join(", "))
}

impl Workspace {
    /// Gives each question of the active chat that waits for an answer its questionnaire.
    pub(super) fn ensure_asks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chats) = &self.claude else { return };
        let chat = chats.active();
        let chat_id = chat.id;
        let missing: Vec<Ask> = chat
            .transcript
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Question(ask)
                    if ask.resolution.is_none() && !chat.asks.contains_key(&ask.request_id) =>
                {
                    Some(ask.clone())
                }
                _ => None,
            })
            .collect();
        for ask in missing {
            let items: Vec<_> = ask
                .questions
                .iter()
                .enumerate()
                .map(|(index, question)| {
                    let typed =
                        cx.new(|cx| InputState::new(window, cx).placeholder("Something else"));
                    QuestionnaireItemDefinition::new(name(index), question.question.clone())
                        .with_description(question.header.clone())
                        .with_multiple(question.multi_select)
                        .with_choices(options(question).into_iter().map(|option| {
                            QuestionnaireChoiceDefinition::new(
                                option.label.clone(),
                                option.label.clone(),
                            )
                        }))
                        .with_input(QuestionnaireInputDefinition::new(typed, "Something else"))
                })
                .collect();
            // only duplicate names or labels fail, and both are made unique above
            let state = cx.new(|cx| QuestionnaireState::new(items, cx).expect("unique names"));
            state.update(cx, |state, cx| {
                state.focus_current_item(window, cx);
            });
            let request_id = ask.request_id.clone();
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this: &mut Workspace, _, event: &QuestionnaireEvent, _, cx| {
                    if let QuestionnaireEvent::Submit(submission) = event {
                        this.claude_answer(chat_id, &request_id, submission, cx);
                    }
                },
            );
            if let Some(chat) = self.claude.as_mut().and_then(|c| c.find(chat_id)) {
                chat.asks.insert(
                    ask.request_id,
                    AskState {
                        state,
                        _subscription: subscription,
                    },
                );
            }
        }
    }

    fn claude_answer(
        &mut self,
        chat_id: u64,
        request_id: &str,
        submission: &QuestionnaireSubmission,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(chat_id)) else {
            return;
        };
        let Some(ask) = chat
            .transcript
            .blocks()
            .iter()
            .find_map(|block| match block {
                Block::Question(ask)
                    if ask.request_id == request_id && ask.resolution.is_none() =>
                {
                    Some(ask.clone())
                }
                _ => None,
            })
        else {
            return;
        };
        let answers: Vec<(String, String)> = ask
            .questions
            .iter()
            .enumerate()
            .filter_map(|(index, question)| {
                let answer = submission.answer(&name(index))?;
                let choices: Vec<String> = answer.choices().iter().map(|c| c.to_string()).collect();
                let text = joined(&choices, answer.freeform().map(|text| text.as_ref()))?;
                Some((question.question.clone(), text))
            })
            .collect();
        let pairs: Vec<(&str, &str)> = answers
            .iter()
            .map(|(question, answer)| (question.as_str(), answer.as_str()))
            .collect();
        let Some(session) = chat.session.as_mut() else {
            return;
        };
        if session.answer(request_id, &ask.input, &pairs).is_ok() {
            chat.transcript
                .resolve(request_id, Resolution::Answered(answers));
        }
        cx.notify();
    }

    fn claude_dismiss(&mut self, chat_id: u64, request_id: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.claude.as_mut().and_then(|c| c.find(chat_id)) else {
            return;
        };
        if let Some(session) = chat.session.as_mut()
            && session
                .deny(request_id, "The user dismissed the question.")
                .is_ok()
        {
            chat.transcript.resolve(request_id, Resolution::Denied);
        }
        cx.notify();
    }

    /// A question: the questionnaire while it waits, what was chosen afterwards.
    pub(super) fn ask_card(&self, chat: &Chat, ask: &Ask, cx: &mut Context<Self>) -> AnyElement {
        let card = div()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(theme::ACTIVE_BORDER))
            .p_3();
        match (&ask.resolution, chat.asks.get(&ask.request_id)) {
            (Some(Resolution::Answered(answers)), _) => card
                .children(answers.iter().map(|(question, answer)| {
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(theme::MUTED))
                                .child(question.clone()),
                        )
                        .child(div().text_color(rgb(theme::STRONG)).child(answer.clone()))
                }))
                .into_any_element(),
            (Some(_), _) => card
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(theme::DIM))
                        .child("Question dismissed"),
                )
                .into_any_element(),
            (None, Some(waiting)) => {
                let st = &waiting.state;
                let (chat_id, request_id) = (chat.id, ask.request_id.clone());
                let items = ask.questions.iter().enumerate().map(|(index, question)| {
                    let item = name(index);
                    let choices = options(question).into_iter().map(|option| {
                        QuestionnaireChoice::new(st, item.clone(), option.label.clone())
                            .child(div().child(option.label.clone()))
                            .child(
                                QuestionnaireChoiceDescription::new()
                                    .child(option.description.clone()),
                            )
                    });
                    QuestionnaireItem::new(st, item.clone())
                        .child(QuestionnaireTitle::new(st, item.clone()))
                        .child(QuestionnaireDescription::new(st, item.clone()))
                        .child(QuestionnaireChoices::new(st, item.clone()).children(choices))
                        .child(QuestionnaireInput::new(st, item.clone()))
                        .child(QuestionnaireError::new(st, item))
                });
                card.child(
                    Questionnaire::new(st)
                        .child(QuestionnaireProgress::new(st))
                        .children(items)
                        .child(
                            QuestionnaireActions::new(st)
                                .child(QuestionnairePrevious::new(st))
                                .child(QuestionnaireSkip::new(st))
                                .child(QuestionnaireNext::new(st))
                                .child(QuestionnaireSubmit::new(st)),
                        ),
                )
                .child(
                    div().flex().child(
                        Button::new(SharedString::from(format!("dismiss-{request_id}")))
                            .ghost()
                            .label("Dismiss")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.claude_dismiss(chat_id, &request_id, cx)
                            })),
                    ),
                )
                .into_any_element()
            }
            (None, None) => card.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use zide_claude::{Question, QuestionOption};

    use super::{joined, name, options};

    fn question(labels: &[&str]) -> Question {
        Question {
            question: "Which?".to_owned(),
            header: "Pick".to_owned(),
            options: labels
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).to_owned(),
                    description: String::new(),
                })
                .collect(),
            multi_select: false,
        }
    }

    #[test]
    fn repeated_or_empty_labels_are_dropped() {
        let question = question(&["A", "B", "A", "", "C"]);
        let labels: Vec<_> = options(&question)
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        assert_eq!(labels, ["A", "B", "C"]);
    }

    #[test]
    fn the_answer_joins_what_was_chosen_and_what_was_typed() {
        let chosen = |labels: &[&str]| labels.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        assert_eq!(joined(&chosen(&["A"]), None).as_deref(), Some("A"));
        assert_eq!(joined(&chosen(&["A", "B"]), None).as_deref(), Some("A, B"));
        assert_eq!(
            joined(&chosen(&[]), Some(" mine ")).as_deref(),
            Some("mine")
        );
        assert_eq!(
            joined(&chosen(&["A"]), Some("more")).as_deref(),
            Some("A, more")
        );
        assert_eq!(joined(&chosen(&[]), Some("  ")), None);
        assert_eq!(name(2), "q2");
    }
}
