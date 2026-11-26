//! The files a commit changed, as the push dialog and the log tab list them: grouped under a
//! line for each folder, colored by what happened to them.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Icon;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::{ChangeKind, FileChange};

use crate::commit::kind_color;
use crate::{file_icon, theme};

const ROW_HEIGHT: f32 = 24.;

/// The files picked in a list, as a file manager does: a plain click picks one, Ctrl+click adds
/// or drops one, Shift picks the range from the last plain click.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picks {
    /// The file the cursor is on.
    pub cursor: usize,
    /// Where a Shift range starts.
    anchor: usize,
    /// The files picked with Ctrl or Shift; empty while only the cursor's is.
    marked: HashSet<usize>,
}

impl Picks {
    /// Every file that is picked.
    pub fn picked(&self) -> HashSet<usize> {
        if self.marked.is_empty() {
            HashSet::from([self.cursor])
        } else {
            self.marked.clone()
        }
    }

    pub fn click(&mut self, index: usize, ctrl: bool, shift: bool) {
        if shift {
            self.marked = (self.anchor.min(index)..=self.anchor.max(index)).collect();
        } else if ctrl {
            let mut picked = self.picked();
            if !picked.remove(&index) {
                picked.insert(index);
            }
            self.marked = picked;
            self.anchor = index;
        } else {
            self.marked.clear();
            self.anchor = index;
        }
        self.cursor = index;
    }

    /// The cursor goes to `to`; with `extend` the files it passes over are picked too.
    pub fn move_to(&mut self, to: usize, extend: bool) {
        if extend {
            self.marked = (self.anchor.min(to)..=self.anchor.max(to)).collect();
        } else {
            self.marked.clear();
            self.anchor = to;
        }
        self.cursor = to;
    }

    /// Picks or drops the file under the cursor, keeping the others.
    pub fn toggle(&mut self) {
        self.click(self.cursor, true, false);
    }
}

/// A line of the files list: a folder that holds files, or one of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRow {
    Dir {
        label: String,
        count: usize,
    },
    File {
        /// Position in the sorted list of files.
        index: usize,
        name: String,
        kind: ChangeKind,
    },
}

/// The folder a path is in, `""` at the top.
fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

pub fn plural(count: usize, one: &str) -> String {
    format!("{count} {one}{}", if count == 1 { "" } else { "s" })
}

/// Sorts by folder, then path: the order [`rows`] and the selection walk.
pub fn sort(list: &mut [FileChange]) {
    list.sort_by(|a, b| (folder(&a.path), &a.path).cmp(&(folder(&b.path), &b.path)));
}

/// The files of a sorted `list` under a line for each folder that holds some. `root` names the
/// folder of the files that sit at the top of the repository.
pub fn rows(list: &[FileChange], root: &str) -> Vec<FileRow> {
    let mut rows: Vec<FileRow> = Vec::new();
    let mut current: Option<&str> = None;
    for (index, change) in list.iter().enumerate() {
        let dir = folder(&change.path);
        if current != Some(dir) {
            current = Some(dir);
            let count = list.iter().filter(|c| folder(&c.path) == dir).count();
            let label = if dir.is_empty() { root } else { dir };
            rows.push(FileRow::Dir {
                label: label.to_string(),
                count,
            });
        }
        let name = change.path.rsplit('/').next().unwrap_or(&change.path);
        rows.push(FileRow::File {
            index,
            name: name.to_string(),
            kind: change.kind,
        });
    }
    rows
}

/// Asked for the menu of a file: which one, and where the mouse is.
pub type OnMenu = Rc<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App)>;

/// The list: `selected` are the files highlighted (brighter when `focused`), `on_click` gets the
/// file clicked, how many clicks in a row it was and the modifier keys held, and `on_menu`, when
/// there is one, is called by a right click.
pub fn list(
    id: &'static str,
    rows: Arc<Vec<FileRow>>,
    selected: Arc<HashSet<usize>>,
    focused: bool,
    scroll: &UniformListScrollHandle,
    on_click: impl Fn(usize, usize, Modifiers, &mut Window, &mut App) + Clone + 'static,
    on_menu: Option<OnMenu>,
) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(
            uniform_list(id, rows.len(), move |range, _, _| {
                range
                    .map(|position| {
                        let row = div()
                            .id((id, position))
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .whitespace_nowrap();
                        match &rows[position] {
                            FileRow::Dir { label, count } => {
                                let (icon, color) = file_icon::for_row(label, true, true);
                                row.child(div().w(px(12.)).child("▾"))
                                    .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                    .child(label.clone())
                                    .child(
                                        div()
                                            .text_color(rgb(theme::DIM))
                                            .child(plural(*count, "file")),
                                    )
                                    .into_any_element()
                            }
                            FileRow::File { index, name, kind } => {
                                let index = *index;
                                let on_click = on_click.clone();
                                let on_menu = on_menu.clone();
                                let (icon, color) = file_icon::for_row(name, false, false);
                                row.pl(px(32.))
                                    .when(selected.contains(&index), |row| {
                                        row.bg(rgb(if focused {
                                            theme::TREE_FOCUSED
                                        } else {
                                            theme::TREE_SELECTED
                                        }))
                                    })
                                    .cursor_pointer()
                                    .on_click(move |event, window, cx| {
                                        on_click(
                                            index,
                                            event.click_count(),
                                            event.modifiers(),
                                            window,
                                            cx,
                                        )
                                    })
                                    .when_some(on_menu, |row, on_menu| {
                                        row.on_mouse_down(
                                            MouseButton::Right,
                                            move |event, window, cx| {
                                                on_menu(index, event.position, window, cx)
                                            },
                                        )
                                    })
                                    .child(Icon::new(icon).size_4().text_color(rgb(color)))
                                    .child(
                                        div()
                                            .text_color(rgb(kind_color(*kind)))
                                            .child(name.clone()),
                                    )
                                    .into_any_element()
                            }
                        }
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(scroll),
        )
        .child(Scrollbar::new(scroll))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use zide_git::{ChangeKind, FileChange};

    use super::{FileRow, Picks, plural, rows, sort};

    fn change(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: path.into(),
            from: None,
            kind,
        }
    }

    #[::core::prelude::v1::test]
    fn files_are_grouped_under_their_folder_in_order() {
        let mut list = vec![
            change("src/b.rs", ChangeKind::Modified),
            change("src/a/deep.rs", ChangeKind::Added),
            change("top.js", ChangeKind::Added),
            change("src/a.rs", ChangeKind::Deleted),
        ];
        sort(&mut list);
        let shown: Vec<String> = rows(&list, "zide")
            .iter()
            .map(|row| match row {
                FileRow::Dir { label, count } => format!("{label}/ {count}"),
                FileRow::File { index, name, .. } => format!("{index}:{name}"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "zide/ 1",
                "0:top.js",
                "src/ 2",
                "1:a.rs",
                "2:b.rs",
                "src/a/ 1",
                "3:deep.rs"
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn the_counts_read_in_singular_and_plural() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(3, "commit"), "3 commits");
    }

    fn picked(picks: &Picks) -> Vec<usize> {
        let mut all: Vec<usize> = picks.picked().into_iter().collect();
        all.sort_unstable();
        all
    }

    #[::core::prelude::v1::test]
    fn ctrl_click_adds_and_drops_files_and_a_plain_click_starts_over() {
        let mut picks = Picks::default();
        picks.click(2, false, false);
        assert_eq!(picked(&picks), [2]);
        picks.click(5, true, false); // keeps the one the cursor was on
        assert_eq!(picked(&picks), [2, 5]);
        picks.click(2, true, false);
        assert_eq!(picked(&picks), [5]);
        picks.click(7, false, false);
        assert_eq!((picked(&picks), picks.cursor), (vec![7], 7));
    }

    #[::core::prelude::v1::test]
    fn shift_picks_the_range_from_the_last_plain_click_whichever_way_it_goes() {
        let mut picks = Picks::default();
        picks.click(3, false, false);
        picks.click(6, false, true);
        assert_eq!(picked(&picks), [3, 4, 5, 6]);
        picks.click(1, false, true); // the anchor stays
        assert_eq!(picked(&picks), [1, 2, 3]);
    }

    #[::core::prelude::v1::test]
    fn shift_with_the_arrows_extends_and_moving_without_it_drops_the_range() {
        let mut picks = Picks::default();
        picks.move_to(1, false);
        picks.move_to(2, true);
        picks.move_to(3, true);
        assert_eq!(picked(&picks), [1, 2, 3]);
        picks.move_to(4, false);
        assert_eq!((picked(&picks), picks.cursor), (vec![4], 4));
        picks.toggle(); // the only one picked: dropping it leaves none marked, the cursor's stays
        picks.move_to(6, false);
        picks.toggle();
        picks.move_to(8, false);
        assert_eq!(picked(&picks), [8]);
    }
}
