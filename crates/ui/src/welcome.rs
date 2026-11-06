//! The screen Zide opens on when it is started without a folder: the projects opened before.

use std::path::{Path, PathBuf};

use gpui_kit::{
    component::{
        IndexPath,
        command::{Command, CommandItem, CommandState},
        input::{Input, InputEvent, InputState},
    },
    *,
};

use crate::{projects, theme};

/// The user picked a project, from the list or with the file picker.
pub struct OpenProject(pub PathBuf);

pub struct Welcome {
    projects: Vec<PathBuf>,
    /// Filters `projects` by what is typed.
    filter: Entity<InputState>,
    /// The list of matching projects.
    commands: Entity<CommandState>,
    /// The highlighted match, as a position in the filtered list.
    selected: usize,
    _subscription: Subscription,
}

impl EventEmitter<OpenProject> for Welcome {}

/// Initials badge colors; a project keeps the same one by its title.
const BADGES: [u32; 8] = [
    0x9b5de5, 0x2ec4b6, 0xd35cc7, 0x5c6bf5, 0x4a90d9, 0x4caf7d, 0xe0803a, 0xd9a21b,
];

impl Welcome {
    /// The input takes the focus, so typing starts filtering right away.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter projects"));
        let subscription = cx.subscribe(
            &filter,
            |this, filter, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.selected = 0; // the first match is the one Enter opens
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let query = filter.read(cx).value().to_string();
                    let found = matching(&this.projects, &query);
                    if let Some(&index) = found.get(this.selected) {
                        cx.emit(OpenProject(this.projects[index].clone()));
                    }
                }
                _ => {}
            },
        );
        filter.update(cx, |state, cx| state.focus(window, cx));
        Self {
            projects: projects::recent(),
            filter,
            commands: cx.new(|cx| CommandState::new(window, cx)),
            selected: 0,
            _subscription: subscription,
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
}

/// A project as a list row: initials badge, title and location.
fn project_item(project: &Path) -> CommandItem {
    let title = projects::title(project);
    let location = projects::location(project);
    let badge = BADGES[title.bytes().map(usize::from).sum::<usize>() % BADGES.len()];
    CommandItem::new().child(move |_, _| {
        div()
            .flex()
            .items_center()
            .gap_3()
            .py_1()
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
                    .child(div().text_color(rgb(theme::STRONG)).child(title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme::DIM))
                            .child(location.clone()),
                    ),
            )
    })
}

/// The indices of the `projects` whose title or location contains `query`, ignoring case.
fn matching(projects: &[PathBuf], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    (0..projects.len())
        .filter(|&i| {
            let project = &projects[i];
            projects::title(project).to_lowercase().contains(&query)
                || projects::location(project).to_lowercase().contains(&query)
        })
        .collect()
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).value().to_string();
        let found = matching(&self.projects, &query);
        self.selected = self.selected.min(found.len().saturating_sub(1));
        let items: Vec<_> = found
            .iter()
            .map(|&i| project_item(&self.projects[i]))
            .collect();
        // follows the selection; deferred so the list has taken this frame's items first
        let state = self.commands.clone();
        let selected = self.selected;
        window.defer(cx, move |window, cx| {
            state.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(selected)), window, cx)
            });
        });
        let (hovered, confirmed) = (cx.entity(), cx.entity());
        let none = if self.projects.is_empty() {
            "No projects yet. Open a folder to start."
        } else {
            "No project matches."
        };
        let list = Command::new(&self.commands)
            .items(items)
            .searchable(false)
            .bordered(false)
            .empty(move |_, _, _| div().p_3().text_color(rgb(theme::DIM)).child(none))
            .on_select(move |path, _, cx| {
                hovered.update(cx, |this, cx| {
                    if this.selected != path.row {
                        this.selected = path.row;
                        cx.notify();
                    }
                });
            })
            .on_confirm(move |path, _, cx| {
                confirmed.update(cx, |this, cx| {
                    let query = this.filter.read(cx).value().to_string();
                    if let Some(&index) = matching(&this.projects, &query).get(path.row) {
                        cx.emit(OpenProject(this.projects[index].clone()));
                    }
                });
            });
        div()
            .id("welcome")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let last = {
                    let query = this.filter.read(cx).value().to_string();
                    matching(&this.projects, &query).len().saturating_sub(1)
                };
                match event.keystroke.key.as_str() {
                    "down" => this.selected = (this.selected + 1).min(last),
                    "up" => this.selected = this.selected.saturating_sub(1),
                    _ => return,
                }
                cx.notify();
            }))
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
                            .items_center()
                            .gap_3()
                            .p_3()
                            .border_b_1()
                            .border_color(rgb(theme::BORDER))
                            .child(div().flex_1().child(Input::new(&self.filter)))
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
                    .child(div().flex_1().min_h_0().p_2().child(list)),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::matching;

    #[test]
    fn the_filter_matches_title_or_location_ignoring_case() {
        let projects = vec![
            PathBuf::from("/srv/Alpha"),
            PathBuf::from("/srv/beta"),
            PathBuf::from("/work/gamma"),
        ];
        assert_eq!(matching(&projects, "ALPHA"), vec![0]);
        assert_eq!(matching(&projects, "srv"), vec![0, 1]);
        assert_eq!(matching(&projects, "  "), vec![0, 1, 2]);
        assert!(matching(&projects, "zeta").is_empty());
    }
}
