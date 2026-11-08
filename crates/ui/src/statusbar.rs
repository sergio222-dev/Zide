//! The bar at the bottom of the window: messages on the left; on the right how the branch
//! stands against its upstream, the branch, and the position and mode of the editor.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::*;
use zide_claude::Overall;
use zide_git::BranchStatus;

use crate::{theme, viewer::ViewerStatus};

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

/// `on_branch` runs when the branch name is clicked; `claude` is `None` when Claude is not
/// installed, and `on_claude` runs when its indicator is clicked.
pub fn render(
    status: Option<ViewerStatus>,
    branch: Option<&BranchStatus>,
    on_branch: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    claude: Option<Overall>,
    on_claude: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> StatusBar {
    let mut bar = StatusBar::new()
        .bg(rgb(theme::BACKGROUND))
        .border_color(rgb(theme::BORDER))
        .text_color(rgb(theme::MUTED));
    if let Some(notice) = status.as_ref().and_then(|s| s.notice.clone()) {
        bar = bar.left(notice);
    }
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
}
