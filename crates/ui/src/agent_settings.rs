//! The Configure Agent dialog: how Claude is started (a sandbox wrapper, say), saved in
//! Zide's settings file.

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable, Sizable as _};
use gpui_kit::*;

use crate::settings::{self, AgentServer};
use crate::{Workspace, theme};

/// The name of an entry saved without one.
pub const DEFAULT_NAME: &str = "Claude";

/// Splits `text` into arguments as a shell would (quotes keep spaces).
pub fn parse_args(text: &str) -> Result<Vec<String>, String> {
    shlex::split(text).ok_or_else(|| "Args: a quote is not closed".to_string())
}

/// `args` as they are typed back: quoted where they need it.
pub fn join_args(args: &[String]) -> String {
    shlex::try_join(args.iter().map(String::as_str)).unwrap_or_else(|_| args.join(" "))
}

/// The entry the form describes; `None` when it names no command (Claude from `PATH`).
pub fn agent_from_form(
    name: &str,
    command: &str,
    args: &str,
    config_dir: &str,
) -> Result<Option<AgentServer>, String> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(None);
    }
    let name = name.trim();
    let config_dir = config_dir.trim();
    Ok(Some(AgentServer {
        name: if name.is_empty() { DEFAULT_NAME } else { name }.to_string(),
        command: command.to_string(),
        args: parse_args(args)?,
        config_dir: (!config_dir.is_empty()).then(|| config_dir.to_string()),
    }))
}

pub struct AgentForm {
    workspace: WeakEntity<Workspace>,
    /// The entry being edited, as the file had it: saving replaces it.
    previous: Option<String>,
    name: Entity<InputState>,
    command: Entity<InputState>,
    args: Entity<InputState>,
    config_dir: Entity<InputState>,
    testing: bool,
    /// The outcome of the last Test or a failed Save, and whether it went well.
    message: Option<(bool, String)>,
}

impl AgentForm {
    fn new(
        workspace: WeakEntity<Workspace>,
        current: Result<Option<AgentServer>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (agent, message) = match current {
            Ok(agent) => (agent.unwrap_or_default(), None),
            // the file is kept as it is; saving will say so again
            Err(why) => (AgentServer::default(), Some((false, why))),
        };
        let mut field = |value: String, placeholder: &str| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder.to_string());
                state.set_value(value, window, cx);
                state
            })
        };

        Self {
            workspace,
            previous: (!agent.name.is_empty()).then(|| agent.name.clone()),
            name: field(agent.name.clone(), DEFAULT_NAME),
            command: field(agent.command.clone(), "claude from PATH"),
            args: field(join_args(&agent.args), "claude"),
            config_dir: field(agent.config_dir.clone().unwrap_or_default(), "~/.claude"),
            testing: false,
            message,
        }
    }

    fn agent(&self, cx: &App) -> Result<Option<AgentServer>, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        agent_from_form(
            &value(&self.name),
            &value(&self.command),
            &value(&self.args),
            &value(&self.config_dir),
        )
    }

    /// Runs `--version` through the command as typed, without saving it.
    fn test(&mut self, cx: &mut Context<Self>) {
        let launcher = match self.agent(cx) {
            Ok(agent) => agent.map(|a| a.launcher()).unwrap_or_default(),
            Err(why) => {
                self.message = Some((false, why));
                return cx.notify();
            }
        };
        self.testing = true;
        self.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { zide_claude::probe(&launcher) })
                .await;
            this.update(cx, |this, cx| {
                this.testing = false;
                this.message = Some(match outcome {
                    Ok(version) if version.is_supported() => (
                        true,
                        format!(
                            "Claude {}.{}.{} answered",
                            version.major, version.minor, version.patch
                        ),
                    ),
                    Ok(version) => (
                        false,
                        format!(
                            "Claude {}.{}.{} is too old (needs {}.{}.{})",
                            version.major,
                            version.minor,
                            version.patch,
                            zide_claude::MIN_VERSION.major,
                            zide_claude::MIN_VERSION.minor,
                            zide_claude::MIN_VERSION.patch
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

    /// Writes the entry and starts new chats with it; open chats keep their process.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.agent(cx).and_then(|agent| {
            let path = settings::path().ok_or("no config folder to save the settings in")?;
            settings::save_agent(&path, self.previous.as_deref(), agent.as_ref())
                .map_err(|e| format!("Could not save {}: {e}", path.display()))?;
            Ok(agent)
        });
        match outcome {
            Ok(agent) => {
                zide_claude::set_launcher(
                    agent
                        .as_ref()
                        .map(AgentServer::launcher)
                        .unwrap_or_default(),
                );
                let said = match &agent {
                    Some(agent) => format!("New chats start Claude with '{}'", agent.name),
                    None => "New chats start Claude from PATH".to_string(),
                };
                self.workspace
                    .update(cx, |workspace, cx| {
                        Workspace::check_claude(cx);
                        workspace.notify(Notification::success(said), cx);
                    })
                    .ok();
                window.close_dialog(cx);
            }
            Err(why) => {
                self.message = Some((false, why));
                cx.notify();
            }
        }
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
        div()
            .flex()
            .flex_col()
            .gap_3()
            .text_sm()
            .child(div().text_color(rgb(theme::MUTED)).child(
                "Zide runs `command args…` followed by Claude's own arguments, so the \
                     command must end up running claude. Leave Command empty to use claude \
                     from PATH.",
            ))
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
                    .child(
                        Button::new("agent-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("agent-save")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

impl Workspace {
    /// The Configure Agent command: edits how Claude is started.
    pub(crate) fn configure_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = match settings::path() {
            Some(path) => settings::load_agent(&path).map_err(|e| e.to_string()),
            None => Ok(None),
        };
        let workspace = cx.entity().downgrade();
        let form = cx.new(|cx| AgentForm::new(workspace, current, window, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Configure Agent")
                .width(px(560.))
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
    fn the_form_becomes_an_entry_or_the_default() {
        assert_eq!(
            agent_from_form("Sandboxed", " ~/bin/a.sh ", "claude", " ~/.s/.claude "),
            Ok(Some(AgentServer {
                name: "Sandboxed".into(),
                command: "~/bin/a.sh".into(),
                args: vec!["claude".into()],
                config_dir: Some("~/.s/.claude".into()),
            }))
        );
        // no name: a generic one; no config dir: none
        let unnamed = agent_from_form(" ", "/bin/a.sh", "", "").unwrap().unwrap();
        assert_eq!(unnamed.name, DEFAULT_NAME);
        assert_eq!(unnamed.config_dir, None);
        // no command: Claude from PATH, the entry is removed
        assert_eq!(agent_from_form("x", "  ", "claude", ""), Ok(None));
        assert!(agent_from_form("x", "/bin/a.sh", "\"oops", "").is_err());
    }
}
