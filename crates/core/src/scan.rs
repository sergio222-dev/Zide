use std::{io, path::Path, sync::mpsc};

use ignore::{WalkBuilder, WalkState};

/// What a scan found, as paths relative to `root` with `/` as the separator, in no particular order.
#[derive(Debug, Default)]
pub struct Scan {
    pub files: Vec<String>,
    /// Directories, including empty ones and excluding `root` itself.
    pub dirs: Vec<String>,
}

/// Folder inside a project where Zide keeps what it remembers about it. It is not part of the
/// project: it is never listed, and so never watched.
pub const STATE_FOLDER: &str = ".zide";

/// Lists the files under `root`, in parallel, honoring `.gitignore` and skipping `.git` and
/// Zide's own folder.
pub fn scan_files(root: &Path) -> io::Result<Vec<String>> {
    scan_project(root, false).map(|scan| scan.files)
}

/// Like [`scan_files`], but also lists the directories (what a watcher has to watch). With
/// `show_ignored`, what git ignores (`.gitignore`, `.git/info/exclude`, the global ignore file,
/// `.ignore`) is listed too; `.git` and Zide's own folder never are.
pub fn scan_project(root: &Path, show_ignored: bool) -> io::Result<Scan> {
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            root.display().to_string(),
        ));
    }
    let (tx, rx) = mpsc::channel::<(bool, String)>();
    WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .standard_filters(false)
        .git_ignore(!show_ignored)
        .git_global(!show_ignored)
        .git_exclude(!show_ignored)
        .ignore(!show_ignored)
        .parents(!show_ignored)
        .filter_entry(|entry| entry.file_name() != ".git" && entry.file_name() != STATE_FOLDER)
        .build_parallel()
        .run(|| {
            let tx = tx.clone();
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                let Some(kind) = entry.file_type() else {
                    return WalkState::Continue;
                };
                if (kind.is_file() || kind.is_dir())
                    && let Ok(relative) = entry.path().strip_prefix(root)
                    && !relative.as_os_str().is_empty()
                {
                    let _ = tx.send((kind.is_dir(), relative.to_string_lossy().replace('\\', "/")));
                }
                WalkState::Continue
            })
        });
    drop(tx);
    let mut scan = Scan::default();
    for (is_dir, path) in rx {
        if is_dir {
            scan.dirs.push(path)
        } else {
            scan.files.push(path)
        }
    }
    Ok(scan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_files_and_honors_gitignore() {
        let dir = std::env::temp_dir().join(format!("zide-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/nested")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        fs::write(dir.join("src/main.rs"), "").unwrap();
        fs::write(dir.join("src/nested/lib.rs"), "").unwrap();
        fs::write(dir.join(".git/HEAD"), "").unwrap();
        fs::write(dir.join("target/out.bin"), "").unwrap();

        let mut files = scan_files(&dir).unwrap();
        files.sort();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(files, [".gitignore", "src/main.rs", "src/nested/lib.rs"]);
    }

    #[test]
    fn show_ignored_lists_what_gitignore_hides_but_never_git() {
        let dir = std::env::temp_dir().join(format!("zide-ignored-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join(".gitignore"), "target/\ndocker-compose.yml\n").unwrap();
        fs::write(dir.join("docker-compose.yml"), "").unwrap();
        fs::write(dir.join("target/out.bin"), "").unwrap();
        fs::write(dir.join(".git/HEAD"), "").unwrap();

        let mut files = scan_project(&dir, true).unwrap().files;
        files.sort();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            files,
            [".gitignore", "docker-compose.yml", "target/out.bin"]
        );
    }

    #[test]
    fn lists_directories_including_empty_ones() {
        let dir = std::env::temp_dir().join(format!("zide-dirs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/empty")).unwrap();
        fs::write(dir.join("src/a.rs"), "").unwrap();
        let mut dirs = scan_project(&dir, false).unwrap().dirs;
        dirs.sort();
        assert_eq!(dirs, ["src", "src/empty"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn zides_own_folder_is_neither_listed_nor_watched() {
        // what Zide writes there (the caret of each file...) must not look like a change of the
        // project: it would reindex everything each time
        let dir = std::env::temp_dir().join(format!("zide-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(STATE_FOLDER)).unwrap();
        fs::write(dir.join(STATE_FOLDER).join(".gitignore"), "*\n").unwrap();
        fs::write(dir.join(STATE_FOLDER).join("views"), "x").unwrap();
        fs::write(dir.join("a.rs"), "").unwrap();
        let scan = scan_project(&dir, false).unwrap();
        assert_eq!(scan.files, ["a.rs"]);
        assert!(scan.dirs.is_empty(), "{:?}", scan.dirs);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_a_missing_root() {
        assert!(scan_files(Path::new("/definitely/not/here")).is_err());
    }
}
