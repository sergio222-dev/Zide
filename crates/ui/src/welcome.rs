//! The screen Zide opens on when it is started without a folder: the projects opened before.

use std::{
    future::Future,
    path::{Path, PathBuf},
};

use gpui_kit::{
    component::{
        IndexPath,
        command::{Command, CommandItem, CommandState},
        input::{Input, InputEvent, InputState},
    },
    *,
};

use crate::{FindAction, projects, theme};

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
    /// The command palette (Ctrl+Shift+A), when open.
    palette: Option<Palette>,
    _subscription: Subscription,
}

/// The commands the welcome screen can run, searched by name.
const COMMANDS: [&str; 2] = ["Open", "Quit"];

struct Palette {
    input: Entity<InputState>,
    commands: Entity<CommandState>,
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
            palette: None,
            _subscription: subscription,
        }
    }

    /// Asks for a folder with the system's picker; picking none changes nothing.
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let picked = choose_folder(cx);
        cx.spawn(async move |this, cx| {
            if let Some(folder) = picked.await {
                this.update(cx, |_, cx| cx.emit(OpenProject(folder))).ok();
            }
        })
        .detach();
    }

    /// Opens the palette, or closes it; the filter gets the focus back when it closes.
    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.filter.update(cx, |state, cx| state.focus(window, cx));
            return cx.notify();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    if let Some(palette) = &mut this.palette {
                        palette.selected = 0;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let query = input.read(cx).value().to_string();
                    let selected = this.palette.as_ref().map_or(0, |p| p.selected);
                    if let Some(&command) = matching_commands(&query).get(selected) {
                        this.run_command(command, window, cx);
                    }
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        self.palette = Some(Palette {
            input,
            commands: cx.new(|cx| CommandState::new(window, cx)),
            selected: 0,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn run_command(&mut self, command: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_palette(window, cx);
        match COMMANDS[command] {
            "Open" => self.pick_folder(cx),
            _ => cx.quit(),
        }
    }

    /// The palette overlay: an input over the commands that match it.
    fn palette_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let palette = self.palette.as_mut()?;
        let query = palette.input.read(cx).value().to_string();
        let found = matching_commands(&query);
        palette.selected = palette.selected.min(found.len().saturating_sub(1));
        let (state, selected) = (palette.commands.clone(), palette.selected);
        window.defer(cx, move |window, cx| {
            state.update(cx, |state, cx| {
                state.set_selected_index(Some(IndexPath::new(selected)), window, cx)
            });
        });
        let items: Vec<_> = found
            .iter()
            .map(|&i| CommandItem::new().child(move |_, _| div().child(COMMANDS[i])))
            .collect();
        let (hovered, confirmed) = (cx.entity(), cx.entity());
        let list = Command::new(&palette.commands)
            .items(items)
            .searchable(false)
            .bordered(false)
            .max_h(px(320.))
            .empty(|_, _, _| div().p_3().text_color(rgb(theme::DIM)).child("No results"))
            .on_select(move |path, _, cx| {
                hovered.update(cx, |this, cx| {
                    if let Some(palette) = this.palette.as_mut().filter(|p| p.selected != path.row)
                    {
                        palette.selected = path.row;
                        cx.notify();
                    }
                });
            })
            .on_confirm(move |path, window, cx| {
                confirmed.update(cx, |this, cx| {
                    let Some(palette) = &this.palette else { return };
                    let query = palette.input.read(cx).value().to_string();
                    if let Some(&command) = matching_commands(&query).get(path.row) {
                        this.run_command(command, window, cx);
                    }
                });
            });
        Some(
            div()
                .absolute()
                .top(px(80.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(480.))
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme::BORDER))
                        .bg(rgb(theme::POPUP))
                        .child(Input::new(&palette.input))
                        .child(list),
                ),
        )
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

/// Asks for a folder with the system's picker; none is picked when it is dismissed.
pub fn choose_folder(cx: &mut App) -> impl Future<Output = Option<PathBuf>> + use<> {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open".into()),
    });
    async move { picked.await.ok()?.ok()??.into_iter().next() }
}

/// The indices of the `COMMANDS` whose name contains `query`, ignoring case.
fn matching_commands(query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    (0..COMMANDS.len())
        .filter(|&i| COMMANDS[i].to_lowercase().contains(&query))
        .collect()
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
        let palette = self.palette_view(window, cx);
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
            .key_context("Welcome")
            .on_action(
                cx.listener(|this, _: &FindAction, window, cx| this.toggle_palette(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if let Some(palette) = &mut this.palette {
                    let query = palette.input.read(cx).value().to_string();
                    let last = matching_commands(&query).len().saturating_sub(1);
                    match key {
                        "down" => palette.selected = (palette.selected + 1).min(last),
                        "up" => palette.selected = palette.selected.saturating_sub(1),
                        "escape" => return this.toggle_palette(window, cx),
                        _ => return,
                    }
                    return cx.notify();
                }
                let last = {
                    let query = this.filter.read(cx).value().to_string();
                    matching(&this.projects, &query).len().saturating_sub(1)
                };
                match key {
                    "down" => this.selected = (this.selected + 1).min(last),
                    "up" => this.selected = this.selected.saturating_sub(1),
                    _ => return,
                }
                cx.notify();
            }))
            .relative()
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
            .children(palette)
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
