//! The files a commit changed, as the push dialog and the log tab list them: grouped under a
//! line for each folder, colored by what happened to them.

use std::sync::Arc;

use gpui_kit::component::Icon;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use zide_git::{ChangeKind, FileChange};

use crate::commit::kind_color;
use crate::{file_icon, theme};

const ROW_HEIGHT: f32 = 24.;

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

/// The list: `selected` is the file highlighted (brighter when `focused`), and `on_click` gets
/// the file clicked and how many clicks in a row it was.
pub fn list(
    id: &'static str,
    rows: Arc<Vec<FileRow>>,
    selected: usize,
    focused: bool,
    scroll: &UniformListScrollHandle,
    on_click: impl Fn(usize, usize, &mut Window, &mut App) + Clone + 'static,
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
                                let (icon, color) = file_icon::for_row(name, false, false);
                                row.pl(px(32.))
                                    .when(index == selected, |row| {
                                        row.bg(rgb(if focused {
                                            theme::TREE_FOCUSED
                                        } else {
                                            theme::TREE_SELECTED
                                        }))
                                    })
                                    .cursor_pointer()
                                    .on_click(move |event, window, cx| {
                                        on_click(index, event.click_count(), window, cx)
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

    use super::{FileRow, plural, rows, sort};

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
}
