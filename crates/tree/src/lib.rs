//! The state of the project tree (file explorer): what is shown, expanded and selected.
//!
//! Ordering follows IntelliJ's Project view: folders always on top, then names compared
//! naturally (`file2` before `file10`) and ignoring case (`AlphaComparator`).

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// A visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Path from the project root, `/`-separated.
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub dir: bool,
    pub expanded: bool,
}

/// What a key or click on the tree asks the workspace to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// Open this file (a path from the project root).
    Open(String),
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    path: String,
    dir: bool,
}

#[derive(Debug, Default)]
pub struct Tree {
    /// Entries of each directory (`""` is the project root), already sorted.
    children: HashMap<String, Vec<Entry>>,
    expanded: HashSet<String>,
    selected: Option<String>,
    rows: Arc<Vec<Row>>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the project contents, keeping what is still there of the expanded folders and
    /// of the selection. `dirs` lists every directory, including empty ones.
    pub fn set_entries(&mut self, files: &[String], dirs: &[String]) {
        let mut children: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut known_dirs: HashSet<&str> = HashSet::new();
        let mut add = |path: &str, dir: bool| {
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            children.entry(parent.to_string()).or_default().push(Entry {
                name: name.to_string(),
                path: path.to_string(),
                dir,
            });
        };
        for dir in dirs {
            if known_dirs.insert(dir) {
                add(dir, true);
            }
        }
        for file in files {
            add(file, false);
            // a file whose folder was not listed still shows under it
            let mut rest = file.as_str();
            while let Some((parent, _)) = rest.rsplit_once('/') {
                if !known_dirs.insert(parent) {
                    break;
                }
                add(parent, true);
                rest = parent;
            }
        }
        for entries in children.values_mut() {
            entries.sort_by(compare_entries);
        }
        self.children = children;
        let children = &self.children;
        self.expanded
            .retain(|path| children.contains_key(path.as_str()));
        if let Some(selected) = &self.selected {
            let (parent, _) = selected.rsplit_once('/').unwrap_or(("", selected));
            let still_there = children
                .get(parent)
                .is_some_and(|entries| entries.iter().any(|e| &e.path == selected));
            if !still_there {
                self.selected = None;
            }
        }
        self.rebuild_rows();
        if self.selected.is_none() && !self.rows.is_empty() {
            self.selected = Some(self.rows[0].path.clone());
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The rows, shared: cheap to hand to a list that renders them later.
    pub fn shared_rows(&self) -> Arc<Vec<Row>> {
        self.rows.clone()
    }

    /// Index of the selected row.
    pub fn selected(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().position(|row| &row.path == selected)
    }

    pub fn select(&mut self, row: usize) {
        if let Some(row) = self.rows.get(row) {
            self.selected = Some(row.path.clone());
        }
    }

    /// Moves the selection `delta` rows, stopping at the ends.
    pub fn move_by(&mut self, delta: isize) {
        let at = self.selected().unwrap_or(0);
        self.select(
            at.saturating_add_signed(delta)
                .min(self.rows.len().saturating_sub(1)),
        );
    }

    /// Ctrl+D / Ctrl+U: moves the selection half of `visible` rows (at least one), stopping at
    /// the ends.
    pub fn move_half_page(&mut self, visible: usize, down: bool) {
        let half = (visible / 2).max(1) as isize;
        self.move_by(if down { half } else { -half });
    }

    pub fn select_first(&mut self) {
        self.select(0);
    }

    pub fn select_last(&mut self) {
        self.select(self.rows.len().saturating_sub(1));
    }

    /// `l` / Right: expands a folder, goes into an expanded one, opens a file.
    pub fn expand(&mut self) -> Outcome {
        let Some(row) = self.selected().map(|i| self.rows[i].clone()) else {
            return Outcome::None;
        };
        if !row.dir {
            return Outcome::Open(row.path);
        }
        if !row.expanded {
            self.set_expanded(&row.path, true);
        } else if self.has_children(&row.path) {
            self.move_by(1);
        }
        Outcome::None
    }

    /// `h` / Left: collapses an expanded folder, otherwise goes to the parent folder.
    pub fn collapse(&mut self) {
        let Some(row) = self.selected().map(|i| self.rows[i].clone()) else {
            return;
        };
        if row.dir && row.expanded {
            self.set_expanded(&row.path, false);
        } else if let Some((parent, _)) = row.path.rsplit_once('/') {
            self.selected = Some(parent.to_string());
        }
    }

    /// Enter: toggles a folder, opens a file.
    pub fn activate(&mut self) -> Outcome {
        let Some(row) = self.selected().map(|i| self.rows[i].clone()) else {
            return Outcome::None;
        };
        if row.dir {
            self.set_expanded(&row.path, !row.expanded);
            Outcome::None
        } else {
            Outcome::Open(row.path)
        }
    }

    /// A click on `row`: selects it, then acts like Enter.
    pub fn click(&mut self, row: usize) -> Outcome {
        self.select(row);
        self.activate()
    }

    /// Expands every folder, as IntelliJ's *Expand All*.
    pub fn expand_all(&mut self) {
        self.expanded = self
            .children
            .values()
            .flatten()
            .filter(|entry| entry.dir)
            .map(|entry| entry.path.clone())
            .collect();
        self.rebuild_rows();
    }

    /// Collapses every folder, as IntelliJ's *Collapse All*. The selection, if it was inside one,
    /// moves to the top folder that held it.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        if let Some(selected) = &mut self.selected
            && let Some((top, _)) = selected.split_once('/')
        {
            *selected = top.to_string();
        }
        self.rebuild_rows();
    }

    /// Expands the folders above `path` and selects it (IntelliJ's *Select Opened File*).
    pub fn reveal(&mut self, path: &str) {
        let mut rest = path;
        let mut changed = false;
        while let Some((parent, _)) = rest.rsplit_once('/') {
            changed |= self.expanded.insert(parent.to_string());
            rest = parent;
        }
        if changed {
            self.rebuild_rows();
        }
        if self.rows.iter().any(|row| row.path == path) {
            self.selected = Some(path.to_string());
        }
    }

    fn has_children(&self, dir: &str) -> bool {
        self.children.get(dir).is_some_and(|c| !c.is_empty())
    }

    fn set_expanded(&mut self, dir: &str, expanded: bool) {
        let changed = if expanded {
            self.expanded.insert(dir.to_string())
        } else {
            self.expanded.remove(dir)
        };
        if changed {
            self.rebuild_rows();
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        // (directory, depth), popped in order
        let mut pending: Vec<(&str, usize)> = vec![("", 0)];
        // depth-first without recursion: push a folder's entries reversed
        let mut stack: Vec<(&Entry, usize)> = Vec::new();
        while let Some((dir, depth)) = pending.pop() {
            if let Some(entries) = self.children.get(dir) {
                stack.extend(entries.iter().rev().map(|e| (e, depth)));
            }
            while let Some((entry, depth)) = stack.pop() {
                let expanded = entry.dir && self.expanded.contains(&entry.path);
                rows.push(Row {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    depth,
                    dir: entry.dir,
                    expanded,
                });
                if expanded && let Some(entries) = self.children.get(&entry.path) {
                    stack.extend(entries.iter().rev().map(|e| (e, depth + 1)));
                }
            }
        }
        self.rows = Arc::new(rows);
    }
}

fn compare_entries(a: &Entry, b: &Entry) -> Ordering {
    b.dir
        .cmp(&a.dir) // folders first
        .then_with(|| natural_compare(&a.name, &b.name))
        .then_with(|| a.name.cmp(&b.name))
}

/// Case-insensitive comparison where runs of digits compare as numbers.
pub fn natural_compare(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (x, y) = (take_number(&mut a), take_number(&mut b));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// The digits at the front of `chars`, without leading zeros.
fn take_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut digits = String::new();
    while let Some(c) = chars.next_if(char::is_ascii_digit) {
        digits.push(c);
    }
    let trimmed = digits.trim_start_matches('0');
    trimmed.to_string()
}

#[cfg(test)]
mod tests;
