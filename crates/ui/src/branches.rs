//! The branch switcher: a popup with the branches of the repository as Recent, Local and
//! Remote, each a tree whose folders are the `/` prefixes of the names (`feat/` holds
//! `feat/x`).

use std::{collections::BTreeMap, collections::HashSet, sync::Arc};

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::*;
use zide_git::{Branches, Switch};

use crate::{Workspace, file_icon, theme};

const ROW_HEIGHT: f32 = 24.;
const SECTIONS: [(&str, &str); 3] = [
    ("recent", "Recent"),
    ("local", "Local"),
    ("remote", "Remote"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Section {
        expanded: bool,
    },
    Folder {
        expanded: bool,
    },
    Branch {
        target: Switch,
        current: bool,
        /// The remote branch a local one tracks, shown beside it.
        upstream: Option<String>,
    },
}

/// A visible line of the switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Identifies the row across rebuilds: `section/path`.
    pub key: String,
    pub depth: usize,
    pub label: String,
    pub kind: Kind,
}

/// A branch of a section, by its full path.
struct Item {
    path: String,
    target: Switch,
    current: bool,
    upstream: Option<String>,
}

/// The state of the switcher without any UI: what is listed, expanded and selected.
pub struct Model {
    data: Branches,
    query: String,
    expanded: HashSet<String>,
    selected: usize,
    rows: Arc<Vec<Row>>,
}

impl Model {
    pub fn new(data: Branches) -> Self {
        let mut model = Self {
            data: Branches::default(),
            query: String::new(),
            expanded: SECTIONS.iter().map(|(key, _)| key.to_string()).collect(),
            selected: 0,
            rows: Arc::default(),
        };
        model.replace(data);
        model
    }

    /// Swaps in newer branches, keeping the query, what the user opened and the selected row
    /// when it is still there. With nothing selected before, the current branch is selected.
    pub fn replace(&mut self, data: Branches) {
        let kept = self.rows.get(self.selected).map(|row| row.key.clone());
        // the folders of the recent branches are open, like the remote names; the folders of
        // the current branch too, so it is in sight
        for name in &data.recent {
            expand_folders(&mut self.expanded, "recent", name);
        }
        for remote in &data.remote {
            if let Some((name, _)) = remote.split_once('/') {
                self.expanded.insert(format!("remote/{name}"));
            }
        }
        if let Some(current) = &data.current {
            expand_folders(&mut self.expanded, "local", current);
        }
        self.data = data;
        self.rebuild();
        self.selected = kept
            .and_then(|key| self.position(&key))
            .or_else(|| {
                let current = self.data.current.clone()?;
                self.position(&format!("local/{current}"))
            })
            .or_else(|| self.first_branch())
            .unwrap_or(0);
    }

    pub fn rows(&self) -> Arc<Vec<Row>> {
        self.rows.clone()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn select(&mut self, row: usize) {
        self.selected = row.min(self.rows.len().saturating_sub(1));
    }

    pub fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }

    /// Lists only the branches whose name has every word of `query`, with their folders open.
    pub fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }
        self.query = query.to_string();
        self.rebuild();
        self.selected = self.first_branch().unwrap_or(0);
    }

    /// Enter or a click: opens or closes a section or a folder; on a branch, what to check out.
    pub fn activate(&mut self) -> Option<Switch> {
        let row = self.rows.get(self.selected)?.clone();
        match row.kind {
            Kind::Branch { target, .. } => Some(target),
            Kind::Section { expanded } | Kind::Folder { expanded } => {
                self.set_expanded(&row.key, !expanded);
                None
            }
        }
    }

    /// Right (`true`) opens the selected folder, Left closes it or goes to its parent folder.
    pub fn fold(&mut self, open: bool) {
        let Some(row) = self.rows.get(self.selected).cloned() else {
            return;
        };
        match row.kind {
            Kind::Section { expanded } | Kind::Folder { expanded } if expanded != open => {
                self.set_expanded(&row.key, open);
            }
            _ if !open => {
                // to the row above that is one level up
                if let Some(parent) = self.rows[..self.selected]
                    .iter()
                    .rposition(|r| r.depth < row.depth)
                {
                    self.selected = parent;
                }
            }
            _ => {}
        }
    }

    fn set_expanded(&mut self, key: &str, open: bool) {
        if open {
            self.expanded.insert(key.to_string());
        } else {
            self.expanded.remove(key);
        }
        self.rebuild();
        self.selected = self.position(key).unwrap_or(0);
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.rows.iter().position(|row| row.key == key)
    }

    fn first_branch(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row.kind, Kind::Branch { .. }))
    }

    fn items(&self, section: &str) -> Vec<Item> {
        let current = self.data.current.as_deref();
        let local = |name: &String| {
            let upstream = self
                .data
                .local
                .iter()
                .find(|b| &b.name == name)
                .and_then(|b| b.upstream.clone());
            Item {
                path: name.clone(),
                target: Switch::Local(name.clone()),
                current: current == Some(name),
                upstream,
            }
        };
        match section {
            "recent" => self.data.recent.iter().map(local).collect(),
            "local" => self.data.local.iter().map(|b| local(&b.name)).collect(),
            _ => self
                .data
                .remote
                .iter()
                .map(|full| Item {
                    path: full.clone(),
                    target: Switch::Remote(full.clone()),
                    current: false,
                    upstream: None,
                })
                .collect(),
        }
    }

    fn rebuild(&mut self) {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let filtering = !words.is_empty();
        let mut rows = Vec::new();
        for (key, label) in SECTIONS {
            let mut items = self.items(key);
            items.retain(|item| {
                let name = item.path.to_lowercase();
                words.iter().all(|word| name.contains(word))
            });
            if items.is_empty() {
                continue;
            }
            let expanded = filtering || self.expanded.contains(key);
            rows.push(Row {
                key: key.to_string(),
                depth: 0,
                label: label.to_string(),
                kind: Kind::Section { expanded },
            });
            if expanded {
                let refs: Vec<&Item> = items.iter().collect();
                self.push_tree(&mut rows, key, "", &refs, 1, filtering);
            }
        }
        self.rows = Arc::new(rows);
    }

    /// The folders then the branches of `items`, whose paths all start with `prefix`.
    fn push_tree(
        &self,
        rows: &mut Vec<Row>,
        section: &str,
        prefix: &str,
        items: &[&Item],
        depth: usize,
        filtering: bool,
    ) {
        let mut folders: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
        let mut leaves: Vec<(&str, &Item)> = Vec::new();
        for &item in items {
            let rest = &item.path[prefix.len()..];
            match rest.split_once('/') {
                Some((folder, _)) => folders.entry(folder).or_default().push(item),
                None => leaves.push((rest, item)),
            }
        }
        for (folder, inside) in folders {
            let key = format!("{section}/{prefix}{folder}");
            let expanded = filtering || self.expanded.contains(&key);
            rows.push(Row {
                key,
                depth,
                label: folder.to_string(),
                kind: Kind::Folder { expanded },
            });
            if expanded {
                let next = format!("{prefix}{folder}/");
                self.push_tree(rows, section, &next, &inside, depth + 1, filtering);
            }
        }
        leaves.sort_by_key(|(name, _)| name.to_lowercase());
        for (name, item) in leaves {
            rows.push(Row {
                key: format!("{section}/{}", item.path),
                depth,
                label: name.to_string(),
                kind: Kind::Branch {
                    target: item.target.clone(),
                    current: item.current,
                    upstream: item.upstream.clone(),
                },
            });
        }
    }
}

/// Marks the folders above `name` (`feat/x/y` → `feat`, `feat/x`) of `section` as open.
fn expand_folders(expanded: &mut HashSet<String>, section: &str, name: &str) {
    let mut path = String::new();
    let mut parts = name.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        path.push_str(part);
        expanded.insert(format!("{section}/{path}"));
        path.push('/');
    }
}

pub struct BranchPopup {
    pub input: Entity<InputState>,
    pub model: Model,
    pub scroll: UniformListScrollHandle,
    /// The branches are being read again; what is listed may be from the last run.
    pub loading: bool,
    pub _subscription: Subscription,
}

impl BranchPopup {
    /// Brings the selected row into view, after the selection moved.
    pub fn reveal_selected(&self) {
        self.scroll
            .scroll_to_item(self.model.selected, ScrollStrategy::Nearest);
    }
}

pub fn render(popup: &BranchPopup, cx: &mut Context<Workspace>) -> Div {
    let rows = popup.model.rows();
    let selected = popup.model.selected();
    let workspace = cx.entity();
    let list = uniform_list("branch-rows", rows.len(), move |range, _, _| {
        range
            .map(|index| {
                let row = &rows[index];
                let workspace = workspace.clone();
                let line = div()
                    .id(("branch-row", index))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(px(8. + 16. * row.depth as f32))
                    .pr_2()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        workspace
                            .update(cx, |this, cx| this.activate_branch_row(index, window, cx));
                    });
                let line = match &row.kind {
                    Kind::Section { expanded } => line
                        .font_weight(FontWeight::BOLD)
                        .child(div().w(px(12.)).child(if *expanded { "▾" } else { "▸" }))
                        .child(row.label.clone()),
                    Kind::Folder { expanded } => {
                        let (icon, color) = file_icon::for_row(&row.label, true, *expanded);
                        line.child(div().w(px(12.)).child(if *expanded { "▾" } else { "▸" }))
                            .child(Icon::new(icon).size_4().text_color(rgb(color)))
                            .child(row.label.clone())
                    }
                    Kind::Branch {
                        current, upstream, ..
                    } => line
                        .child(div().w(px(12.)))
                        .child(
                            div()
                                .w(px(16.))
                                .text_color(rgb(theme::ICON_FOLDER))
                                .child(if *current { "★" } else { "" }),
                        )
                        .child(row.label.clone())
                        .children(upstream.clone().map(|upstream| {
                            div()
                                .ml_auto()
                                .text_xs()
                                .text_color(rgb(theme::DIM))
                                .child(upstream)
                        })),
                };
                if index == selected {
                    line.bg(rgb(theme::TREE_FOCUSED))
                } else {
                    line
                }
            })
            .collect()
    })
    .track_scroll(&popup.scroll)
    .flex_1()
    .min_h_0();
    div()
        .absolute()
        .top_16()
        .left_1_4()
        .w_1_2()
        .h(px(440.))
        .p_2()
        .gap_2()
        .flex()
        .flex_col()
        .bg(rgb(theme::POPUP))
        .border_1()
        .border_color(rgb(theme::CONTROL_BORDER))
        .rounded_md()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().child(Input::new(&popup.input)))
                .children(popup.loading.then(|| {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(rgb(theme::MUTED))
                        .child(Spinner::new().small().color(rgb(theme::MUTED).into()))
                        .child("Loading")
                })),
        )
        .child(list)
}

#[cfg(test)]
mod tests {
    use zide_git::LocalBranch;

    use super::*;

    fn local(name: &str, upstream: Option<&str>) -> LocalBranch {
        LocalBranch {
            name: name.into(),
            upstream: upstream.map(str::to_string),
        }
    }

    fn data() -> Branches {
        Branches {
            current: Some("feat/card".into()),
            local: vec![
                local("main", Some("origin/main")),
                local("feat/card", None),
                local("feat/a/deep", None),
            ],
            remote: vec!["origin/main".into(), "origin/feat/card".into()],
            recent: vec!["feat/card".into(), "main".into()],
        }
    }

    /// `depth label`, with `/` after folders and sections.
    fn shown(model: &Model) -> Vec<String> {
        model
            .rows()
            .iter()
            .map(|row| {
                let mark = match row.kind {
                    Kind::Branch { .. } => "",
                    _ => "/",
                };
                format!("{}{}{mark}", "  ".repeat(row.depth), row.label)
            })
            .collect()
    }

    #[::core::prelude::v1::test]
    fn branches_are_grouped_by_section_with_folders_for_their_prefixes() {
        let model = Model::new(data());
        assert_eq!(
            shown(&model),
            [
                "Recent/",
                "  feat/", // recent folders start open
                "    card",
                "  main",
                "Local/",
                "  feat/", // the current branch's folder is open, `a` is not
                "    a/",
                "    card",
                "  main",
                "Remote/",
                "  origin/", // remote names start open, their folders do not
                "    feat/",
                "    main",
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn the_current_local_branch_starts_selected() {
        let model = Model::new(data());
        let rows = model.rows();
        assert_eq!(rows[model.selected()].key, "local/feat/card");
    }

    #[::core::prelude::v1::test]
    fn enter_toggles_folders_and_checks_out_branches() {
        let mut model = Model::new(data());
        let at = |model: &Model, key: &str| model.position(key).unwrap();
        model.select(at(&model, "local/feat/a"));
        assert_eq!(model.activate(), None); // opens `a`
        assert!(shown(&model).contains(&"      deep".to_string()));
        assert_eq!(model.activate(), None); // and closes it again
        assert!(!shown(&model).contains(&"      deep".to_string()));

        model.select(at(&model, "remote/origin/main"));
        assert_eq!(model.activate(), Some(Switch::Remote("origin/main".into())));
        model.select(at(&model, "local/main"));
        assert_eq!(model.activate(), Some(Switch::Local("main".into())));
    }

    #[::core::prelude::v1::test]
    fn left_closes_a_folder_then_goes_to_its_parent() {
        let mut model = Model::new(data());
        model.select(model.position("local/feat/card").unwrap());
        model.fold(false); // a branch: up to its folder
        assert_eq!(model.rows()[model.selected()].key, "local/feat");
        model.fold(false); // the folder closes
        assert_eq!(model.position("local/feat/card"), None);
        model.fold(false); // closed: up to the section
        assert_eq!(model.rows()[model.selected()].key, "local");
        model.fold(true);
        model.fold(true);
        assert_eq!(model.rows()[model.selected()].key, "local");
    }

    #[::core::prelude::v1::test]
    fn the_query_keeps_matching_branches_with_every_folder_open() {
        let mut model = Model::new(data());
        model.set_query("DEEP");
        assert_eq!(shown(&model), ["Local/", "  feat/", "    a/", "      deep"]);
        assert_eq!(model.rows()[model.selected()].key, "local/feat/a/deep");
        model.set_query("origin card");
        assert_eq!(
            shown(&model),
            ["Remote/", "  origin/", "    feat/", "      card"]
        );
        model.set_query("nothing");
        assert!(model.rows().is_empty());
        model.set_query("");
        assert_eq!(shown(&model).len(), 13);
    }

    #[::core::prelude::v1::test]
    fn newer_branches_keep_the_query_and_the_selected_row() {
        // an empty cache: nothing listed, then the real branches arrive and the current is selected
        let mut model = Model::new(Branches::default());
        assert!(model.rows().is_empty());
        model.replace(data());
        assert_eq!(model.rows()[model.selected()].key, "local/feat/card");

        model.select(model.position("local/main").unwrap());
        let mut newer = data();
        newer.local.push(local("zeta", None));
        model.replace(newer);
        assert_eq!(model.rows()[model.selected()].key, "local/main");
        assert!(model.position("local/zeta").is_some());

        model.set_query("zeta");
        let mut newest = data();
        newest.local.push(local("zeta", None));
        model.replace(newest);
        assert_eq!(shown(&model), ["Local/", "  zeta"]);
    }

    #[::core::prelude::v1::test]
    fn a_local_branch_shows_the_remote_it_tracks() {
        let model = Model::new(data());
        let rows = model.rows();
        let main = rows.iter().find(|r| r.key == "local/main").unwrap();
        assert!(matches!(&main.kind, Kind::Branch { upstream: Some(u), .. } if u == "origin/main"));
    }
}
