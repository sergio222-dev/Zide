use std::{io, path::Path, sync::mpsc};

use ignore::{WalkBuilder, WalkState};

/// Lists the files under `root`, in parallel, honoring `.gitignore` and skipping `.git`.
///
/// Paths are relative to `root`, with `/` as the separator, in no particular order.
pub fn scan_files(root: &Path) -> io::Result<Vec<String>> {
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            root.display().to_string(),
        ));
    }
    let (tx, rx) = mpsc::channel::<String>();
    WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let tx = tx.clone();
            Box::new(move |entry| {
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if entry.file_type().is_some_and(|t| t.is_file())
                    && let Ok(relative) = entry.path().strip_prefix(root)
                {
                    let _ = tx.send(relative.to_string_lossy().replace('\\', "/"));
                }
                WalkState::Continue
            })
        });
    drop(tx);
    Ok(rx.into_iter().collect())
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
    fn rejects_a_missing_root() {
        assert!(scan_files(Path::new("/definitely/not/here")).is_err());
    }
}
