//! Zide's own folder of language servers, like Mason's in Neovim: a server found there wins
//! over the one on the `PATH`, and installing into it touches nothing else on the machine.
//!
//! Layout: `<folder>/bin/<command>` for programs, `<folder>/node_modules/.bin/<command>` for
//! what `npm install --prefix <folder> <package>` installs.

use std::path::{Path, PathBuf};

use crate::ServerSpec;

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

/// What to run for `spec`: its native build when Zide's folder has one, else the program
/// from [`resolve`] with the usual arguments.
pub fn launch(spec: &ServerSpec) -> Launch {
    let native = spec.native.as_ref().and_then(|native| {
        let found = (native.find)(&servers_dir()?)?;
        Some(Launch {
            program: found,
            args: native.args,
            native: true,
        })
    });
    native.unwrap_or_else(|| Launch {
        program: resolve(spec.command),
        args: spec.args,
        native: false,
    })
}

/// How a server is run, as chosen by [`launch`].
pub struct Launch {
    pub program: PathBuf,
    pub args: &'static [&'static str],
    pub native: bool,
}

/// The `rust-glancer` binary in Zide's folder; unlike the other servers it is never taken from
/// the `PATH`, so that having it installed elsewhere does not swap rust-analyzer out.
pub fn native_rust(folder: &Path) -> Option<PathBuf> {
    find_in(folder, "rust-glancer")
}

/// The TypeScript 7 executable that npm installs as `@typescript/typescript-<os>-<arch>`.
pub fn native_typescript(folder: &Path) -> Option<PathBuf> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let exe = if cfg!(windows) { "tsc.exe" } else { "tsc" };
    let path = folder
        .join("node_modules/@typescript")
        .join(format!("typescript-{os}-{arch}"))
        .join("lib")
        .join(exe);
    path.is_file().then_some(path)
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
    fn the_native_typescript_is_found_by_platform() {
        let dir = folder("native");
        assert_eq!(native_typescript(&dir), None);
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        let lib = dir.join(format!(
            "node_modules/@typescript/typescript-{os}-{arch}/lib"
        ));
        fs::create_dir_all(&lib).unwrap();
        let exe = lib.join(if cfg!(windows) { "tsc.exe" } else { "tsc" });
        fs::write(&exe, "").unwrap();
        assert_eq!(native_typescript(&dir), Some(exe));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rust_glancer_is_found_only_in_zides_folder() {
        let dir = folder("glancer");
        assert_eq!(native_rust(&dir), None);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/rust-glancer"), "").unwrap();
        assert_eq!(native_rust(&dir), Some(dir.join("bin/rust-glancer")));
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
