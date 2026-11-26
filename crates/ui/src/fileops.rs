//! What the project tree does to files: move, delete and copy, always inside the project.
//! Paths are relative to the project root and `/`-separated; the empty path is the root.

use std::fs;
use std::path::{Path, PathBuf};

/// The path as it will be used: no leading or trailing `/`, no empty parts. Refuses what leaves
/// the project (`..`) or means nothing (`.`).
pub fn normalize(typed: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for part in typed.trim().split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." {
            return Err("The path must stay inside the project".into());
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn absolute(root: &Path, relative: &str) -> PathBuf {
    root.join(relative)
}

/// The new path for `from` when the user typed `typed`, or why it is not good.
pub fn check_move(root: &Path, from: &str, typed: &str) -> Result<String, String> {
    let to = normalize(typed)?;
    if to.is_empty() {
        return Err("The path is empty".into());
    }
    if to == from {
        return Ok(to);
    }
    if to
        .strip_prefix(from)
        .is_some_and(|rest| rest.starts_with('/'))
    {
        return Err("A folder cannot be moved into itself".into());
    }
    if absolute(root, &to).symlink_metadata().is_ok() {
        return Err(format!("'{to}' already exists"));
    }
    Ok(to)
}

/// What a name typed for a new entry means: its path and whether it is a folder (the name ends
/// with `/`), or why it is not good.
pub fn check_new(root: &Path, typed: &str) -> Result<(String, bool), String> {
    let folder = typed.trim().ends_with('/');
    let path = normalize(typed)?;
    if path.is_empty() {
        return Err("The name is empty".into());
    }
    let mut at = String::new();
    for part in path.split('/') {
        if !at.is_empty() {
            at.push('/');
        }
        at.push_str(part);
        match absolute(root, &at).symlink_metadata() {
            Ok(_) if at == path => return Err(format!("'{path}' already exists")),
            Ok(meta) if !meta.is_dir() => return Err(format!("'{at}' is a file")),
            _ => {}
        }
    }
    Ok((path, folder))
}

/// Creates the file or, when `typed` ends with `/`, the folder, with the folders on the way.
/// Returns its path and whether it is a folder.
pub fn create(root: &Path, typed: &str) -> Result<(String, bool), String> {
    let (path, folder) = check_new(root, typed)?;
    let target = absolute(root, &path);
    if folder {
        fs::create_dir_all(&target).map_err(|e| format!("Could not create the folder: {e}"))?;
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Could not create the folder: {e}"))?;
        }
        fs::File::create_new(&target).map_err(|e| format!("Could not create the file: {e}"))?;
    }
    Ok((path, folder))
}

/// Moves `from` to `to` (both relative), creating the folders `to` needs.
pub fn move_path(root: &Path, from: &str, to: &str) -> Result<(), String> {
    let to = check_move(root, from, to)?;
    if to == from {
        return Ok(());
    }
    let target = absolute(root, &to);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Could not create the folder: {e}"))?;
    }
    fs::rename(absolute(root, from), target).map_err(|e| format!("Could not move it: {e}"))
}

/// Deletes a file, a link or a folder with everything in it. Never leaves the project.
pub fn delete(root: &Path, path: &str) -> Result<(), String> {
    let path = normalize(path)?;
    if path.is_empty() {
        return Err("The project itself cannot be deleted".into());
    }
    let target = absolute(root, &path);
    let kind = target
        .symlink_metadata()
        .map_err(|e| format!("Could not read '{path}': {e}"))?;
    let removed = if kind.is_dir() {
        fs::remove_dir_all(&target)
    } else {
        fs::remove_file(&target)
    };
    removed.map_err(|e| format!("Could not delete '{path}': {e}"))
}

/// The first name next to `name` that is free in `dir`: `a.rs`, `a copy.rs`, `a copy 2.rs`...
fn free_name(dir: &Path, name: &str) -> String {
    if dir.join(name).symlink_metadata().is_err() {
        return name.to_string();
    }
    let (stem, extension) = match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };
    let mut copy = format!("{stem} copy{extension}");
    let mut number = 2;
    while dir.join(&copy).symlink_metadata().is_ok() {
        copy = format!("{stem} copy {number}{extension}");
        number += 1;
    }
    copy
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let kind = from.symlink_metadata()?;
    if kind.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else if kind.is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
        #[cfg(not(unix))]
        fs::copy(from, to).map(drop)
    } else {
        fs::copy(from, to).map(drop)
    }
}

/// Copies `source` into the folder `dest_dir` (the root when empty) under a free name; gives the
/// path of the copy.
pub fn copy_into(root: &Path, source: &str, dest_dir: &str) -> Result<String, String> {
    let (source, dest_dir) = (normalize(source)?, normalize(dest_dir)?);
    if source.is_empty() {
        return Err("The project itself cannot be copied".into());
    }
    if dest_dir == source
        || dest_dir
            .strip_prefix(&source)
            .is_some_and(|rest| rest.starts_with('/'))
    {
        return Err("A folder cannot be pasted into itself".into());
    }
    let from = absolute(root, &source);
    if from.symlink_metadata().is_err() {
        return Err(format!("'{source}' no longer exists"));
    }
    let dir = absolute(root, &dest_dir);
    let name = source.rsplit('/').next().unwrap_or(&source);
    let name = free_name(&dir, name);
    copy_tree(&from, &dir.join(&name)).map_err(|e| format!("Could not copy '{source}': {e}"))?;
    Ok(if dest_dir.is_empty() {
        name
    } else {
        format!("{dest_dir}/{name}")
    })
}

/// Where `path` lands when `from` (a file or a folder) is moved to `to`: itself when it is not
/// inside `from`.
pub fn follow(path: &str, from: &str, to: &str) -> Option<String> {
    if path == from {
        return Some(to.to_string());
    }
    path.strip_prefix(from)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(|rest| format!("{to}/{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-fileops-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/deep")).unwrap();
        fs::write(dir.join("src/a.rs"), "a").unwrap();
        fs::write(dir.join("src/deep/b.rs"), "b").unwrap();
        fs::write(dir.join("top.txt"), "t").unwrap();
        dir
    }

    #[::core::prelude::v1::test]
    fn paths_are_cleaned_and_cannot_leave_the_project() {
        assert_eq!(normalize("  /a//b/ ").unwrap(), "a/b");
        assert!(normalize("a/../b").is_err());
        assert!(normalize("./a").is_err());
        assert_eq!(normalize("").unwrap(), "");
    }

    #[::core::prelude::v1::test]
    fn a_name_ending_with_a_slash_makes_a_folder_and_any_other_a_file() {
        let dir = project("new");
        assert_eq!(create(&dir, "src/new/").unwrap(), ("src/new".into(), true));
        assert!(dir.join("src/new").is_dir());
        assert_eq!(
            create(&dir, "docs/deep/n.md").unwrap(),
            ("docs/deep/n.md".into(), false)
        );
        assert!(dir.join("docs/deep/n.md").is_file());
        assert!(
            create(&dir, "top.txt")
                .unwrap_err()
                .contains("already exists")
        );
        assert!(create(&dir, "top.txt/x").unwrap_err().contains("is a file"));
        assert!(create(&dir, "../x").is_err());
        assert!(create(&dir, "  ").is_err());
        assert!(!dir.join("top.txt/x").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[::core::prelude::v1::test]
    fn moving_renames_creates_folders_and_refuses_taken_or_nested_targets() {
        let dir = project("move");
        assert_eq!(
            check_move(&dir, "top.txt", " "),
            Err("The path is empty".into())
        );
        assert!(
            check_move(&dir, "top.txt", "src/a.rs")
                .unwrap_err()
                .contains("already exists")
        );
        assert!(
            check_move(&dir, "src", "src/deep/x")
                .unwrap_err()
                .contains("into itself")
        );
        assert!(check_move(&dir, "top.txt", "../out").is_err());
        assert_eq!(check_move(&dir, "top.txt", "top.txt").unwrap(), "top.txt");

        move_path(&dir, "top.txt", "new/nested/renamed.txt").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("new/nested/renamed.txt")).unwrap(),
            "t"
        );
        assert!(!dir.join("top.txt").exists());
        move_path(&dir, "src", "lib").unwrap(); // a folder moves with its files
        assert!(dir.join("lib/deep/b.rs").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[::core::prelude::v1::test]
    fn delete_removes_files_and_whole_folders_but_not_the_project() {
        let dir = project("delete");
        delete(&dir, "top.txt").unwrap();
        delete(&dir, "src").unwrap();
        assert!(!dir.join("top.txt").exists() && !dir.join("src").exists());
        assert!(delete(&dir, "").is_err());
        assert!(delete(&dir, "../x").is_err());
        assert!(delete(&dir, "missing").is_err());
        assert!(dir.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[::core::prelude::v1::test]
    fn copies_get_a_free_name_and_folders_go_whole() {
        let dir = project("copy");
        assert_eq!(copy_into(&dir, "top.txt", "").unwrap(), "top copy.txt");
        assert_eq!(copy_into(&dir, "top.txt", "").unwrap(), "top copy 2.txt");
        assert_eq!(copy_into(&dir, "top.txt", "src").unwrap(), "src/top.txt");
        assert_eq!(copy_into(&dir, "src", "").unwrap(), "src copy");
        assert!(dir.join("src copy/deep/b.rs").exists());
        assert!(
            copy_into(&dir, "src", "src/deep")
                .unwrap_err()
                .contains("into itself")
        );
        assert!(dir.join("top.txt").exists()); // the original stays
        let _ = fs::remove_dir_all(dir);
    }

    #[::core::prelude::v1::test]
    fn a_path_follows_the_move_of_its_folder() {
        assert_eq!(
            follow("src/a.rs", "src", "lib").as_deref(),
            Some("lib/a.rs")
        );
        assert_eq!(follow("src", "src", "lib").as_deref(), Some("lib"));
        assert_eq!(follow("srcx/a.rs", "src", "lib"), None);
    }
}
