//! The bar at the bottom of the window: messages on the left; on the right how the branch
//! stands against its upstream, the branch, and the position and mode of the editor.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{Sizable as _, spinner::Spinner, tooltip::Tooltip};
use gpui_kit::*;
use zide_claude::Overall;
use zide_git::BranchStatus;

use crate::{ai, theme, viewer::ViewerStatus};

/// The icon of the branch name, shipped with the app: the kit has none.
pub const BRANCH_ICON: &str = "icons/git-branch.svg";

/// How the branch stands against its upstream, and the color to show it in: the commits to push
/// (`↑`), those to pull (`↓`) and, when both exist (the branches diverged), the net difference.
pub fn branch_label(branch: &BranchStatus) -> (String, u32) {
    match (branch.tracked, branch.ahead, branch.behind) {
        (false, ..) => ("no upstream".into(), theme::DIM),
        (true, 0, 0) => ("up-to-date".into(), theme::MUTED),
        (true, ahead, 0) => (format!("↑{ahead}"), theme::CHANGE_ADDED),
        (true, 0, behind) => (format!("↓{behind}"), theme::CARET_BAR),
        (true, ahead, behind) => (
            format!(
                "↑{ahead} ↓{behind} Δ{:+}",
                i64::from(ahead) - i64::from(behind)
            ),
            theme::ICON_FOLDER,
        ),
    }
}

/// The dot of the Claude indicator: gray without a process, orange while any chat thinks,
/// green when none does and one waits.
pub fn claude_color(overall: Overall) -> u32 {
    match overall {
        Overall::Off => theme::DIM,
        Overall::Working => theme::WORKING,
        Overall::Waiting => theme::OK,
    }
}

/// Where the language server of the open file stands, as the status bar needs it: the workspace
/// keeps the real state, this only borrows what is shown.
pub enum Server<'a> {
    /// Never asked for, or it died: the next request starts it again.
    Stopped,
    Starting,
    /// Running; what to call it (its command, or `<name> (native)`).
    Ready(&'a str),
    /// It could not run: why, with the command that installs it.
    Unavailable(&'a str),
}

/// The language server indicator: a dot in a color and a name, and what hovering it explains.
pub struct ServerIndicator {
    pub label: String,
    pub color: u32,
    pub tooltip: String,
}

/// The indicator for the server whose command is `command`, in the same colors as the AI one:
/// gray when not running, orange while starting, green when ready, red when it cannot run.
pub fn server_indicator(command: &str, server: Server) -> ServerIndicator {
    let (label, color, tooltip) = match server {
        Server::Stopped => (
            command.to_string(),
            theme::DIM,
            "Language server not running: it starts on the next request".to_string(),
        ),
        Server::Starting => (
            format!("{command} starting…"),
            theme::WORKING,
            "Language server starting".to_string(),
        ),
        Server::Ready(label) => (
            label.to_string(),
            theme::OK,
            "Language server running".to_string(),
        ),
        Server::Unavailable(why) => (command.to_string(), theme::ERROR, why.to_string()),
    };
    ServerIndicator {
        label,
        color,
        tooltip,
    }
}

/// `busy` names the background work in progress, if any. `on_branch` runs when the branch name is clicked; `claude` is `None` when Claude is not
/// installed, and `on_claude` runs when its indicator is clicked. `ai` is the state of the local
/// model, and `on_ai` turns it on or off. `server` is the open file's language server, `None`
/// when its language has none.
#[allow(clippy::too_many_arguments)]
pub fn render(
    status: Option<ViewerStatus>,
    busy: Option<&'static str>,
    server: Option<ServerIndicator>,
    branch: Option<&BranchStatus>,
    on_branch: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    claude: Option<Overall>,
    on_claude: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ai: &ai::Ai,
    on_ai: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> StatusBar {
    let mut bar = StatusBar::new()
        .bg(rgb(theme::BACKGROUND))
        .border_color(rgb(theme::BORDER))
        .text_color(rgb(theme::MUTED));
    if let Some(notice) = status.as_ref().and_then(|s| s.notice.clone()) {
        bar = bar.left(notice);
    }
    // something runs in the background (git, an update): a spinner so it does not look frozen
    if let Some(label) = busy {
        bar = bar.left(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Spinner::new().small().color(rgb(theme::WORKING).into()))
                .child(label),
        );
    }
    if let Some(server) = server {
        let tooltip = SharedString::from(server.tooltip);
        bar = bar.right(
            div()
                .id("language-server")
                .flex()
                .items_center()
                .gap_1()
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .child(div().size_2().rounded_full().bg(rgb(server.color)))
                .child(server.label),
        );
    }
    bar = bar.right(
        div()
            .id("ai")
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .on_click(on_ai)
            .child(div().size_2().rounded_full().bg(rgb(ai.color())))
            .child(ai.label()),
    );
    if let Some(overall) = claude {
        bar = bar.right(
            div()
                .id("claude")
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .on_click(on_claude)
                .child(div().size_2().rounded_full().bg(rgb(claude_color(overall))))
                .child("Claude"),
        );
    }
    if let Some(branch) = branch {
        let (label, color) = branch_label(branch);
        // an operation that is not finished: the reminder that it still needs a commit
        if let Some(progress) = &branch.progress {
            bar = bar.right(
                div()
                    .text_color(rgb(theme::WORKING))
                    .child(progress.label()),
            );
        }
        bar = bar.right(div().text_color(rgb(color)).child(label)).right(
            div()
                .id("branch")
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .on_click(on_branch)
                .child(
                    svg()
                        .path(BRANCH_ICON)
                        .size_3()
                        .text_color(rgb(theme::MUTED)),
                )
                .child(branch.name.clone()),
        );
    }
    if let Some(status) = status {
        bar = bar
            .right(format!("{}:{}", status.line, status.column))
            .right(
                div()
                    .flex()
                    .gap_2()
                    .child(status.mode)
                    .child(status.pending)
                    .child(if status.dirty { "[+]" } else { "" }),
            );
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(tracked: bool, ahead: u32, behind: u32) -> String {
        let branch = BranchStatus {
            name: "main".into(),
            tracked,
            ahead,
            behind,
            progress: None,
        };
        branch_label(&branch).0
    }

    #[::core::prelude::v1::test]
    fn the_label_says_what_to_push_what_to_pull_and_how_far_apart_they_are() {
        assert_eq!(label(false, 0, 0), "no upstream");
        assert_eq!(label(true, 0, 0), "up-to-date");
        assert_eq!(label(true, 4, 0), "↑4");
        assert_eq!(label(true, 0, 2), "↓2");
        assert_eq!(label(true, 4, 2), "↑4 ↓2 Δ+2");
        assert_eq!(label(true, 1, 3), "↑1 ↓3 Δ-2");
    }

    #[::core::prelude::v1::test]
    fn the_server_indicator_names_the_server_and_colors_its_state() {
        let stopped = server_indicator("pyright-langserver", Server::Stopped);
        assert_eq!(
            (stopped.label.as_str(), stopped.color),
            ("pyright-langserver", theme::DIM)
        );

        let starting = server_indicator("pyright-langserver", Server::Starting);
        assert_eq!(
            (starting.label.as_str(), starting.color),
            ("pyright-langserver starting…", theme::WORKING)
        );

        // the running server's own name wins: a native build is not the spec's command
        let ready = server_indicator("rust-analyzer", Server::Ready("rust (native)"));
        assert_eq!(
            (ready.label.as_str(), ready.color),
            ("rust (native)", theme::OK)
        );

        let why = "pyright-langserver is not installed. Setup: npm install pyright";
        let missing = server_indicator("pyright-langserver", Server::Unavailable(why));
        assert_eq!(
            (
                missing.label.as_str(),
                missing.color,
                missing.tooltip.as_str()
            ),
            ("pyright-langserver", theme::ERROR, why)
        );
    }
}
