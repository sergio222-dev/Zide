//! The Configure Agent dialog: the agents Claude can be started through (a sandbox wrapper,
//! say), which one is in use, and each one's command; saved in Zide's settings file.

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Sizable as _};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::Value;

use crate::settings::{self, AgentServer, Protocol};
use crate::{Workspace, theme};

/// The name of an entry saved without one.
pub const DEFAULT_NAME: &str = "Agent";

/// Splits `text` into arguments as a shell would (quotes keep spaces).
pub fn parse_args(text: &str) -> Result<Vec<String>, String> {
    shlex::split(text).ok_or_else(|| "Args: a quote is not closed".to_string())
}

/// `args` as they are typed back: quoted where they need it.
pub fn join_args(args: &[String]) -> String {
    shlex::try_join(args.iter().map(String::as_str)).unwrap_or_else(|_| args.join(" "))
}

/// The entry the form describes.
pub fn agent_from_form(
    name: &str,
    command: &str,
    args: &str,
    config_dir: &str,
    protocol: Protocol,
) -> Result<AgentServer, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("Command is empty: name the script or program that runs claude".into());
    }
    let name = name.trim();
    let config_dir = config_dir.trim();
    Ok(AgentServer {
        name: if name.is_empty() { DEFAULT_NAME } else { name }.to_string(),
        command: command.to_string(),
        args: parse_args(args)?,
        config_dir: (!config_dir.is_empty()).then(|| config_dir.to_string()),
        protocol,
    })
}

/// What the dialog shows on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selected {
    /// Claude from `PATH`: nothing to edit.
    Path,
    Agent(String),
    New,
}

pub struct AgentForm {
    workspace: WeakEntity<Workspace>,
    /// The settings as last read or written.
    file: Value,
    selected: Selected,
    name: Entity<InputState>,
    command: Entity<InputState>,
    args: Entity<InputState>,
    config_dir: Entity<InputState>,
    /// The protocol picked for the selected agent.
    protocol: Protocol,
    testing: bool,
    /// The outcome of the last action, and whether it went well.
    message: Option<(bool, String)>,
}

impl AgentForm {
    fn new(
        workspace: WeakEntity<Workspace>,
        file: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut field = |placeholder: &str| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()))
        };
        let mut form = Self {
            workspace,
            file: Value::Null,
            selected: Selected::Path,
            protocol: Protocol::Claude,
            name: field(DEFAULT_NAME),
            command: field("/path/to/agent-sandbox.sh"),
            args: field("claude"),
            config_dir: field("~/.claude"),
            testing: false,
            message: None,
        };
        match file {
            Ok(file) => {
                form.file = file;
                let selected = match settings::active(&form.file) {
                    Some(agent) => Selected::Agent(agent.name),
                    None => Selected::Path,
                };
                form.select(selected, window, cx);
            }
            // the file is kept as it is: every change will say so again
            Err(why) => form.message = Some((false, why)),
        }
        form
    }

    /// Shows `selected` on the right, its fields filled in.
    fn select(&mut self, selected: Selected, window: &mut Window, cx: &mut Context<Self>) {
        let agent = match &selected {
            Selected::Agent(name) => settings::agents(&self.file)
                .into_iter()
                .find(|agent| &agent.name == name)
                .unwrap_or_default(),
            _ => AgentServer::default(),
        };
        let values = [
            (&self.name, agent.name.clone()),
            (&self.command, agent.command.clone()),
            (&self.args, join_args(&agent.args)),
            (
                &self.config_dir,
                agent.config_dir.clone().unwrap_or_default(),
            ),
        ];
        for (input, value) in values {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        self.protocol = agent.protocol;
        self.selected = selected;
        self.message = None;
        cx.notify();
    }

    fn form(&self, cx: &App) -> Result<AgentServer, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        agent_from_form(
            &value(&self.name),
            &value(&self.command),
            &value(&self.args),
            &value(&self.config_dir),
            self.protocol,
        )
    }

    /// Changes the settings file with `change`; on success Claude starts as it now says (new
    /// chats only), and `done` is shown.
    fn change(
        &mut self,
        change: impl FnOnce(Value) -> Value,
        done: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let outcome = settings::path()
            .ok_or_else(|| "no config folder to save the settings in".to_string())
            .and_then(|path| {
                settings::update(&path, change)
                    .map_err(|e| format!("Could not save {}: {e}", path.display()))
            });
        match outcome {
            Ok(file) => {
                settings::use_file(&file);
                self.file = file;
                self.workspace
                    .update(cx, |_, cx| Workspace::check_claude(cx))
                    .ok();
                self.message = Some((true, done));
                cx.notify();
                true
            }
            Err(why) => {
                self.message = Some((false, why));
                cx.notify();
                false
            }
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent = match self.form(cx) {
            Ok(agent) => agent,
            Err(why) => {
                self.message = Some((false, why));
                return cx.notify();
            }
        };
        let previous = match &self.selected {
            Selected::Agent(name) => Some(name.clone()),
            _ => None,
        };
        let taken = settings::agents(&self.file)
            .iter()
            .any(|other| other.name == agent.name && Some(&other.name) != previous.as_ref());
        if taken {
            self.message = Some((
                false,
                format!("There is already an agent named '{}'", agent.name),
            ));
            return cx.notify();
        }
        let name = agent.name.clone();
        let saved = self.change(
            |file| settings::with_agent(file, previous.as_deref(), &agent),
            String::new(),
            cx,
        );
        if saved {
            self.select(Selected::Agent(name.clone()), window, cx);
            let in_use = settings::active(&self.file).is_some_and(|a| a.name == name);
            self.message = Some((
                true,
                if in_use {
                    format!("Saved: new chats start Claude with '{name}'")
                } else {
                    format!("Saved '{name}'. Use it to start new chats with it")
                },
            ));
        }
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Selected::Agent(name) = self.selected.clone() else {
            return;
        };
        let file = |file| settings::without_agent(file, &name);
        if self.change(file, String::new(), cx) {
            let selected = match settings::active(&self.file) {
                Some(agent) => Selected::Agent(agent.name),
                None => Selected::Path,
            };
            self.select(selected, window, cx);
            self.message = Some((true, format!("Deleted '{name}'")));
        }
    }

    /// Makes the selected agent (or Claude from `PATH`) the one new chats start.
    fn use_selected(&mut self, cx: &mut Context<Self>) {
        let name = match &self.selected {
            Selected::Agent(name) => Some(name.clone()),
            Selected::Path => None,
            Selected::New => return,
        };
        let done = match &name {
            Some(name) => format!("New chats start Claude with '{name}'"),
            None => "New chats start Claude from PATH".to_string(),
        };
        self.change(
            |file| settings::with_active(file, name.as_deref()),
            done,
            cx,
        );
    }

    /// Checks what the form says (unsaved), or `PATH`'s claude: `--version` for Claude, the
    /// `initialize` handshake for an ACP agent.
    fn test(&mut self, cx: &mut Context<Self>) {
        let (launcher, protocol) = match &self.selected {
            Selected::Path => (zide_claude::Launcher::default(), Protocol::Claude),
            _ => match self.form(cx) {
                Ok(agent) => (agent.launcher(), agent.protocol),
                Err(why) => {
                    self.message = Some((false, why));
                    return cx.notify();
                }
            },
        };
        self.testing = true;
        self.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    match protocol {
                        Protocol::Claude => Ok(zide_claude::probe(&launcher)),
                        Protocol::Acp => Err(zide_acp::probe(&launcher)),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.testing = false;
                let text = |v: zide_claude::Version| format!("{}.{}.{}", v.major, v.minor, v.patch);
                let outcome = match outcome {
                    Ok(claude) => claude,
                    Err(acp) => {
                        this.message = Some(match acp {
                            Ok(agent) => (true, format!("{agent} answered")),
                            Err(why) => (false, why),
                        });
                        return cx.notify();
                    }
                };
                this.message = Some(match outcome {
                    Ok(version) if version.is_supported() => {
                        (true, format!("Claude {} answered", text(version)))
                    }
                    Ok(version) => (
                        false,
                        format!(
                            "Claude {} is too old (needs {})",
                            text(version),
                            text(zide_claude::MIN_VERSION)
                        ),
                    ),
                    Err(why) => (false, why),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Claude Code's stream-json, or the Agent Client Protocol other agents speak.
    fn protocol_row(&self, cx: &mut Context<Self>) -> Div {
        let choice = |id: &'static str, label: &'static str, protocol: Protocol| {
            let button = Button::new(id).label(label).on_click(cx.listener(
                move |this: &mut Self, _, _, cx| {
                    this.protocol = protocol;
                    this.message = None;
                    cx.notify();
                },
            ));
            if self.protocol == protocol {
                button.primary()
            } else {
                button.ghost()
            }
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_color(rgb(theme::TEXT)).child("Protocol"))
            .child(choice("protocol-claude", "Claude Code", Protocol::Claude))
            .child(choice("protocol-acp", "ACP", Protocol::Acp))
            .child(
                div()
                    .text_color(rgb(theme::DIM))
                    .child(match self.protocol {
                        Protocol::Claude => "the command runs claude",
                        Protocol::Acp => "e.g. copilot --acp --stdio",
                    }),
            )
    }

    /// Whether `selected` is the one new chats start with.
    fn in_use(&self, selected: &Selected) -> bool {
        let active = settings::active(&self.file).map(|agent| agent.name);
        match selected {
            Selected::Path => active.is_none(),
            Selected::Agent(name) => active.as_ref() == Some(name),
            Selected::New => false,
        }
    }

    fn list_row(
        &self,
        id: usize,
        label: String,
        selected: Selected,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.selected == selected;
        let in_use = self.in_use(&selected);
        div()
            .id(("agent", id))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(if current { theme::STRONG } else { theme::TEXT }))
            .when(current, |row| row.bg(rgb(theme::ACTIVE_BG)))
            .child(
                div()
                    .size_2()
                    .rounded_full()
                    .when(in_use, |dot| dot.bg(rgb(theme::OK))),
            )
            .child(div().flex_1().truncate().child(label))
            .on_click(
                cx.listener(move |this, _, window, cx| this.select(selected.clone(), window, cx)),
            )
    }
}

/// A label over its input.
fn row(label: &'static str, hint: &'static str, input: &Entity<InputState>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().text_color(rgb(theme::TEXT)).child(label))
                .child(div().text_color(rgb(theme::DIM)).child(hint)),
        )
        .child(Input::new(input))
}

impl Render for AgentForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agents = settings::agents(&self.file);
        let editing = self.selected != Selected::Path;
        let saved = matches!(self.selected, Selected::Agent(_));
        let in_use = self.in_use(&self.selected);

        let mut list = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(200.))
            .pr_3()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .child(self.list_row(0, "Claude (PATH)".into(), Selected::Path, cx));
        for (i, agent) in agents.into_iter().enumerate() {
            let selected = Selected::Agent(agent.name.clone());
            list = list.child(self.list_row(i + 1, agent.name, selected, cx));
        }
        list = list.child(
            Button::new("agent-new")
                .label("+ New Agent")
                .ghost()
                .on_click(
                    cx.listener(|this, _, window, cx| this.select(Selected::New, window, cx)),
                ),
        );

        let details = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_color(rgb(theme::MUTED)).child(if !editing {
                "Claude found on PATH (or its usual install places), with its own config in \
                 ~/.claude."
            } else if self.protocol == Protocol::Acp {
                "Zide runs `command args…` and speaks the Agent Client Protocol on its stdin \
                 and stdout. Claude's gauges, model and effort, history and background agents \
                 are not offered for it."
            } else {
                "Zide runs `command args…` followed by Claude's own arguments, so the command \
                 must end up running claude."
            }))
            .when(editing, |details| {
                details
                    .child(self.protocol_row(cx))
                    .child(row("Name", "", &self.name))
                    .child(row(
                        "Command",
                        "a wrapper script, e.g. a sandbox",
                        &self.command,
                    ))
                    .child(row("Args", "before Claude's, e.g. claude", &self.args))
                    .child(row(
                        "Config dir",
                        "where that Claude keeps its sessions",
                        &self.config_dir,
                    ))
            })
            .children(self.message.as_ref().map(|(ok, text)| {
                div()
                    .text_color(rgb(if *ok { theme::OK } else { theme::ERROR }))
                    .child(text.clone())
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap_2()
                    .children(
                        self.testing
                            .then(|| Spinner::new().small().color(rgb(theme::MUTED).into())),
                    )
                    .child(
                        Button::new("agent-test")
                            .label("Test")
                            .disabled(self.testing)
                            .on_click(cx.listener(|this, _, _, cx| this.test(cx))),
                    )
                    .when(saved, |buttons| {
                        buttons.child(
                            Button::new("agent-delete")
                                .label("Delete")
                                .danger()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.delete(window, cx)),
                                ),
                        )
                    })
                    .when(self.selected != Selected::New, |buttons| {
                        buttons.child(
                            Button::new("agent-use")
                                .label(if in_use { "In Use" } else { "Use" })
                                .disabled(in_use)
                                .on_click(cx.listener(|this, _, _, cx| this.use_selected(cx))),
                        )
                    })
                    .when(editing, |buttons| {
                        buttons.child(
                            Button::new("agent-save")
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        )
                    })
                    .child(
                        Button::new("agent-close")
                            .label("Close")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            );

        div()
            .flex()
            .gap_3()
            .text_sm()
            .min_h(px(320.))
            .child(list)
            .child(details)
    }
}

impl Workspace {
    /// The Configure Agent command: edits how Claude is started.
    pub(crate) fn configure_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = match settings::path() {
            Some(path) => settings::read(&path).map_err(|e| e.to_string()),
            None => Ok(serde_json::json!({})),
        };
        let workspace = cx.entity().downgrade();
        let form = cx.new(|cx| AgentForm::new(workspace, file, window, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Configure Agent")
                .width(px(760.))
                .child(form.clone())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn args_are_split_like_a_shell_does() {
        assert_eq!(
            parse_args("claude --x"),
            Ok(vec!["claude".into(), "--x".into()])
        );
        assert_eq!(
            parse_args(r#"claude "/a path/with spaces""#),
            Ok(vec!["claude".into(), "/a path/with spaces".into()])
        );
        assert_eq!(parse_args("   "), Ok(vec![]));
        assert!(parse_args(r#"claude "unclosed"#).is_err());
        let args = vec!["claude".to_string(), "/a path".to_string()];
        assert_eq!(parse_args(&join_args(&args)), Ok(args));
    }

    #[::core::prelude::v1::test]
    fn the_form_becomes_an_entry() {
        assert_eq!(
            agent_from_form(
                "Sandboxed",
                " ~/bin/a.sh ",
                "claude",
                " ~/.s/.claude ",
                Protocol::Claude
            ),
            Ok(AgentServer {
                name: "Sandboxed".into(),
                command: "~/bin/a.sh".into(),
                args: vec!["claude".into()],
                config_dir: Some("~/.s/.claude".into()),
                ..AgentServer::default()
            })
        );
        // no name: a generic one; no config dir: none
        let unnamed = agent_from_form(" ", "/bin/a.sh", "", "", Protocol::Claude).unwrap();
        assert_eq!(unnamed.name, DEFAULT_NAME);
        assert_eq!(unnamed.config_dir, None);
        // an agent needs a command; quotes must close
        assert!(agent_from_form("x", "  ", "claude", "", Protocol::Claude).is_err());
        assert!(agent_from_form("x", "/bin/a.sh", "\"oops", "", Protocol::Claude).is_err());
        // the protocol picked is kept
        let copilot = agent_from_form("Copilot", "copilot", "--acp --stdio", "", Protocol::Acp);
        assert_eq!(copilot.unwrap().protocol, Protocol::Acp);
    }
}
