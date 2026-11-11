//! The box under the conversation: the input with its send/stop button, and under it what the
//! chat runs with. Model, effort and context each open a small panel over their button.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Chat, Chats, footer, gauges, options};
use crate::{Workspace, theme};

/// The look every panel over the buttons shares, in Zide's own colors.
fn panel(width: f32) -> Div {
    div()
        .w(px(width))
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(rgb(theme::CONTROL_BORDER))
        .bg(rgb(theme::POPUP))
        .shadow_lg()
        .text_sm()
        .text_color(rgb(theme::TEXT))
}

impl Workspace {
    /// The input box and the line of settings under it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn composer(
        &self,
        chat: &Chat,
        chats: &Chats,
        menu: Option<Div>,
        images: Option<Div>,
        bare: bool,
        background: usize,
        now: u64,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .max_w(px(900.))
            .mx_auto()
            .flex()
            .flex_col()
            .gap_1()
            .children(menu)
            .child(
                div()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(theme::CONTROL_BORDER))
                    .bg(rgb(theme::POPUP))
                    .px_3()
                    .py_2()
                    .children(images)
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(
                                Textarea::new(&chat.input).appearance(false).bordered(false),
                            ))
                            .child(self.send_or_stop(chat, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_1()
                    .text_xs()
                    .text_color(rgb(theme::DIM))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(footer(chats.status_of(chat), &chat.transcript, background))
                            // loud on purpose: this chat is not running with the user's setup
                            .children(bare.then(|| {
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(theme::ERROR))
                                    .child("BARE MODE")
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            // to the left of the model, in the colour of the usage bar
                            // limits, models, effort and context are Claude Code's own
                            .when(chat.protocol == crate::settings::Protocol::Claude, |row| {
                                row.children(gauges::warning(chats.limits.as_ref(), now).map(
                                    |(color, text)| {
                                        div()
                                            .px_2()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(color))
                                            .child(text)
                                    },
                                ))
                                .child(self.model_menu(chat, &chats.defaults, cx))
                                .child(self.effort_menu(chat, &chats.defaults, cx))
                                .child(self.context_menu(chat, chats, now))
                            }),
                    ),
            )
    }

    /// The button beside the input: Stop while Claude works (Esc presses it), else Send.
    pub(super) fn send_or_stop(&self, chat: &Chat, cx: &mut Context<Self>) -> Button {
        let id = chat.id;
        if chat.busy() {
            Button::new("claude-stop")
                .ghost()
                .label("■")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.claude_interrupt();
                    cx.notify();
                }))
        } else {
            Button::new("claude-send")
                .ghost()
                .label("↵")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let text = this
                        .claude
                        .as_mut()
                        .and_then(|c| c.find(id))
                        .map(|chat| chat.input.read(cx).value().to_string());
                    if let Some(text) = text {
                        this.claude_send(id, text, window, cx);
                    }
                }))
        }
    }

    /// The model in use; its panel lists the models with a check on the chosen one.
    fn model_menu(
        &self,
        chat: &Chat,
        defaults: &zide_claude::Defaults,
        cx: &mut Context<Self>,
    ) -> Popover {
        let workspace = cx.weak_entity();
        let id = chat.id;
        let rows = options::model_rows(defaults, chat.model.as_deref());
        let selected = options::model_row(chat.model.as_deref(), &rows);
        Popover::new("claude-model")
            .anchor(Anchor::BottomRight)
            .appearance(false)
            .trigger(
                Button::new("claude-model-button")
                    .ghost()
                    .xsmall()
                    .label(options::model_label(chat.model.as_deref(), defaults)),
            )
            .content(move |_, _, cx| {
                let popover = cx.entity();
                panel(240.).children(rows.iter().map(|row| {
                    let (workspace, popover, name) =
                        (workspace.clone(), popover.clone(), row.clone());
                    div()
                        .id(SharedString::from(format!("model-{row}")))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|row| row.bg(rgb(theme::TREE_SELECTED)))
                        .child(row.clone())
                        .children(
                            (*row == selected)
                                .then(|| Icon::new(IconName::Check).text_color(rgb(theme::OK))),
                        )
                        .on_click(move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.claude_pick_model(id, &name, cx))
                                .ok();
                            popover.update(cx, |state, cx| state.dismiss(window, cx));
                        })
                }))
            })
    }

    /// The effort in use; its panel has a slider from faster to smarter.
    fn effort_menu(
        &self,
        chat: &Chat,
        defaults: &zide_claude::Defaults,
        cx: &mut Context<Self>,
    ) -> Popover {
        let workspace = cx.weak_entity();
        let id = chat.id;
        let label = options::effort_label(chat.effort.as_deref(), defaults);
        let slider = chat.effort_slider.clone();
        let reset = options::effort_rows(defaults)[0].clone();
        let chosen = chat.effort.is_some();
        Popover::new("claude-effort")
            .anchor(Anchor::BottomRight)
            .appearance(false)
            .trigger(
                Button::new("claude-effort-button")
                    .ghost()
                    .xsmall()
                    .label(label.clone()),
            )
            .content(move |_, _, _| {
                let (workspace, reset) = (workspace.clone(), reset.clone());
                panel(280.)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().text_color(rgb(theme::MUTED)).child("Effort"))
                            .child(div().text_color(rgb(theme::STRONG)).child(label.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(theme::MUTED))
                            .child("Faster")
                            .child("Smarter"),
                    )
                    .child(Slider::new(&slider))
                    .child(
                        div()
                            .id("claude-effort-default")
                            .text_xs()
                            .text_color(rgb(theme::DIM))
                            .when(chosen, |hint| {
                                hint.cursor_pointer()
                                    .hover(|hint| hint.text_color(rgb(theme::STRONG)))
                                    .on_click(move |_, _, cx| {
                                        let reset = reset.clone();
                                        workspace
                                            .update(cx, |this, cx| {
                                                this.claude_pick_effort(id, &reset, cx)
                                            })
                                            .ok();
                                    })
                            })
                            .child(if chosen {
                                "Use the default"
                            } else {
                                "Recommended"
                            }),
                    )
            })
    }

    /// A ring that fills with the context; its panel has the window and the plan's limits.
    fn context_menu(&self, chat: &Chat, chats: &Chats, now: u64) -> Popover {
        let share = gauges::context_share(&chat.transcript, &chats.defaults).unwrap_or(0.0);
        let color = gauges::color(share);
        let transcript = chat.transcript.clone();
        let limits = chats.limits;
        let defaults = chats.defaults.clone();
        Popover::new("claude-context-panel")
            .anchor(Anchor::BottomRight)
            .appearance(false)
            .trigger(
                Button::new("claude-context-button").ghost().xsmall().child(
                    ProgressCircle::new("claude-context-ring")
                        .value((share.clamp(0.0, 1.0) * 100.0) as f32)
                        .color(rgb(color))
                        .size(px(14.)),
                ),
            )
            .content(move |_, _, _| {
                panel(300.).p_4().child(gauges::report(
                    &transcript,
                    &defaults,
                    limits.as_ref(),
                    now,
                ))
            })
    }
}
