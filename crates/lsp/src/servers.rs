//! Zide's own folder of language servers, like Mason's in Neovim: a server found there wins
//! over the one on the `PATH`, and installing into it touches nothing else on the machine.
//!
//! Layout: `<folder>/bin/<command>` for programs, `<folder>/node_modules/.bin/<command>` for
//! what `npm install --prefix <folder> <package>` installs.

use std::path::{Path, PathBuf};

/// `~/.local/share/zide/servers` on Linux; the data folder of the platform elsewhere.
pub fn servers_dir() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("zide").join("servers"))
}

/// Where `command` is in `folder`, if it was installed there.
pub fn find_in(folder: &Path, command: &str) -> Option<PathBuf> {
    [
        folder.join("bin").join(command),
        folder.join("node_modules").join(".bin").join(command),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// The program to run for `command`: the one in Zide's folder, else the bare name, which
/// the system looks up on the `PATH`.
pub fn resolve(command: &str) -> PathBuf {
    servers_dir()
        .and_then(|folder| find_in(&folder, command))
        .unwrap_or_else(|| PathBuf::from(command))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zide-servers-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn programs_are_found_in_bin_and_in_npm_bin() {
        let dir = folder("find");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("node_modules/.bin")).unwrap();
        fs::write(dir.join("bin/rust-analyzer"), "").unwrap();
        fs::write(dir.join("node_modules/.bin/typescript-language-server"), "").unwrap();
        assert_eq!(
            find_in(&dir, "rust-analyzer"),
            Some(dir.join("bin/rust-analyzer"))
        );
        assert_eq!(
            find_in(&dir, "typescript-language-server"),
            Some(dir.join("node_modules/.bin/typescript-language-server"))
        );
        assert_eq!(find_in(&dir, "gopls"), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bin_wins_over_npm_and_folders_do_not_count() {
        let dir = folder("order");
        fs::create_dir_all(dir.join("bin/imposter")).unwrap(); // a directory, not a program
        fs::create_dir_all(dir.join("node_modules/.bin")).unwrap();
        fs::write(dir.join("bin/tool"), "").unwrap();
        fs::write(dir.join("node_modules/.bin/tool"), "").unwrap();
        assert_eq!(find_in(&dir, "tool"), Some(dir.join("bin/tool")));
        assert_eq!(find_in(&dir, "imposter"), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_program_resolves_to_its_bare_name() {
        assert_eq!(
            resolve("zide-no-such-language-server"),
            PathBuf::from("zide-no-such-language-server")
        );
    }
}
