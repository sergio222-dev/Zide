//! Tells when the set of project files may have changed (ADR: index stays fresh).
//!
//! Watches every directory that holds an indexed file, non-recursively, so ignored trees
//! (`target/`, `node_modules/`, ...) never cost an inotify watch. A new directory shows up
//! as a change in its parent; the caller rescans and calls [`watch_project`] again.

use std::{path::Path, sync::mpsc, thread, time::Duration};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};

/// How long to wait for more events before reporting a burst as one change.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// What changed. `Files` implies the contents may have changed too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// Only the contents of existing files.
    Contents,
    /// Files were created, removed or renamed: the file list is stale.
    Files,
}

/// Keeps watching until dropped.
pub struct ProjectWatcher {
    _watcher: RecommendedWatcher,
    /// Directories that could not be watched (e.g. inotify watch limit reached).
    pub unwatched: usize,
}

/// Calls `on_change` (from a background thread) after files were created, removed, renamed or
/// written in `root` or in any of `dirs` (the relative, `/`-separated directories from
/// [`scan_project`](crate::scan_project)). Bursts of events produce a single call.
pub fn watch_project(
    root: &Path,
    dirs: &[String],
    on_change: impl Fn(Change) + Send + 'static,
) -> notify::Result<ProjectWatcher> {
    let (tx, rx) = mpsc::channel::<Change>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Some(change) = event.ok().and_then(|e| classify(&e.kind)) {
            let _ = tx.send(change);
        }
    })?;

    let mut unwatched = 0;
    for dir in std::iter::once("").chain(dirs.iter().map(String::as_str)) {
        if watcher
            .watch(&root.join(dir), RecursiveMode::NonRecursive)
            .is_err()
        {
            unwatched += 1;
        }
    }

    thread::spawn(move || {
        // ends when the watcher (and with it the sender) is dropped
        while let Ok(mut change) = rx.recv() {
            while let Ok(more) = rx.recv_timeout(DEBOUNCE) {
                change = change.max(more);
            }
            on_change(change);
        }
    });
    Ok(ProjectWatcher {
        _watcher: watcher,
        unwatched,
    })
}

fn classify(kind: &EventKind) -> Option<Change> {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) => {
            Some(Change::Files)
        }
        EventKind::Modify(ModifyKind::Data(_)) => Some(Change::Contents),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::mpsc::channel};

    fn temp_project(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "").unwrap();
        dir
    }

    #[test]
    fn reports_a_created_file_once_per_burst() {
        let root = temp_project("create");
        let (tx, rx) = channel();
        let _watcher = watch_project(&root, &["src".to_string()], move |change| {
            let _ = tx.send(change);
        })
        .unwrap();

        for i in 0..5 {
            fs::write(root.join(format!("src/new{i}.rs")), "").unwrap();
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Change::Files
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "burst split in two"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reports_edits_to_file_contents_as_content_changes() {
        let root = temp_project("edit");
        let (tx, rx) = channel();
        let _watcher = watch_project(&root, &["src".to_string()], move |change| {
            let _ = tx.send(change);
        })
        .unwrap();

        fs::write(root.join("src/a.rs"), "changed").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Change::Contents
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reports_a_removed_file() {
        let root = temp_project("remove");
        let (tx, rx) = channel();
        let _watcher = watch_project(&root, &["src".to_string()], move |change| {
            let _ = tx.send(change);
        })
        .unwrap();

        fs::remove_file(root.join("src/a.rs")).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Change::Files
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
