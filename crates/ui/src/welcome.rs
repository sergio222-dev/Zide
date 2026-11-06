//! The screen Zide opens on when it is started without a folder: the projects opened before.

use std::path::{Path, PathBuf};

use gpui_kit::*;

use crate::{projects, theme};

/// The user picked a project, from the list or with the file picker.
pub struct OpenProject(pub PathBuf);

pub struct Welcome {
    projects: Vec<PathBuf>,
}

impl EventEmitter<OpenProject> for Welcome {}

/// Initials badge colors; a project keeps the same one by its title.
const BADGES: [u32; 8] = [
    0x9b5de5, 0x2ec4b6, 0xd35cc7, 0x5c6bf5, 0x4a90d9, 0x4caf7d, 0xe0803a, 0xd9a21b,
];

impl Welcome {
    pub fn new() -> Self {
        Self {
            projects: projects::recent(),
        }
    }

    /// Asks for a folder with the system's picker; picking none changes nothing.
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(folder) = paths.into_iter().next()
            {
                this.update(cx, |_, cx| cx.emit(OpenProject(folder))).ok();
            }
        })
        .detach();
    }

    fn row(&self, index: usize, project: &Path, cx: &mut Context<Self>) -> Stateful<Div> {
        let title = projects::title(project);
        let badge = BADGES[title.bytes().map(usize::from).sum::<usize>() % BADGES.len()];
        let folder = project.to_path_buf();
        div()
            .id(("project", index))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|row| row.bg(rgb(theme::TREE_SELECTED)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded_md()
                    .bg(rgb(badge))
                    .text_xs()
                    .text_color(rgb(theme::BACKGROUND))
                    .child(projects::initials(&title)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_color(rgb(theme::STRONG)).child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::DIM))
                            .child(projects::location(project)),
                    ),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenProject(folder.clone()))))
    }
}

impl Render for Welcome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .map(|(i, project)| self.row(i, project, cx))
            .collect();
        let list = if rows.is_empty() {
            div()
                .p_3()
                .text_color(rgb(theme::DIM))
                .child("No projects yet. Open a folder to start.")
                .into_any_element()
        } else {
            div()
                .id("projects")
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .size_full()
                .px_2()
                .children(rows)
                .into_any_element()
        };
        div()
            .id("welcome")
            .flex()
            .size_full()
            .bg(rgb(theme::BACKGROUND))
            .text_color(rgb(theme::TEXT))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(230.))
                    .h_full()
                    .flex_none()
                    .p_2()
                    .bg(rgb(theme::POPUP))
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(theme::ACTIVE_BG))
                            .text_color(rgb(theme::STRONG))
                            .child("Projects"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .p_3()
                            .border_b_1()
                            .border_color(rgb(theme::BORDER))
                            .child(
                                div()
                                    .id("open")
                                    .px_3()
                                    .py_1()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme::CONTROL_BORDER))
                                    .cursor_pointer()
                                    .hover(|b| b.bg(rgb(theme::TREE_SELECTED)))
                                    .child("Open")
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                            ),
                    )
                    .child(div().flex_1().min_h_0().pt_2().child(list)),
            )
    }
}
