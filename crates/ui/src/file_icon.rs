//! The icon of a row of the project tree: folders by state, files by extension.

use gpui_kit::assets::{Assets, IconName, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

use crate::theme;

icon_assets!(
    TreeIcons,
    [
        Folder,
        FolderOpen,
        File,
        FileCode,
        FileBraces,
        FileCog,
        FileLock,
        FileText,
        FileImage,
        FileArchive
    ]
);

/// The icons of the tree on top of the kit's default ones.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == crate::statusbar::BRANCH_ICON {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../icons/git-branch.svg"
            ))));
        }
        match TreeIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = TreeIcons.list(path)?;
        if crate::statusbar::BRANCH_ICON.starts_with(path) {
            paths.push(crate::statusbar::BRANCH_ICON.into());
        }
        paths.extend(Assets.list(path)?);
        Ok(paths)
    }
}

/// The icon and its color for a tree row named `name`.
pub fn for_row(name: &str, dir: bool, expanded: bool) -> (IconName, u32) {
    if dir {
        let icon = if expanded {
            IconName::FolderOpen
        } else {
            IconName::Folder
        };
        return (icon, theme::ICON_FOLDER);
    }
    // a dotfile such as `.gitignore` has no extension: the part after the dot is its kind
    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "go" | "py" | "java" | "c" | "h"
        | "cpp" | "hpp" | "cs" | "rb" | "php" | "sh" | "html" | "css" | "scss" | "vue"
        | "svelte" => (IconName::FileCode, theme::ICON_CODE),
        "json" => (IconName::FileBraces, theme::ICON_DATA),
        "toml" | "yaml" | "yml" | "ini" | "env" | "gitignore" => {
            (IconName::FileCog, theme::ICON_DATA)
        }
        "lock" => (IconName::FileLock, theme::ICON_DATA),
        "md" | "txt" => (IconName::FileText, theme::MUTED),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => {
            (IconName::FileImage, theme::ICON_MEDIA)
        }
        "zip" | "tar" | "gz" | "xz" => (IconName::FileArchive, theme::MUTED),
        _ => (IconName::File, theme::MUTED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(name: &str) -> IconName {
        for_row(name, false, false).0
    }

    #[test]
    fn a_folder_shows_whether_it_is_open() {
        assert!(matches!(for_row("src", true, false).0, IconName::Folder));
        assert!(matches!(for_row("src", true, true).0, IconName::FolderOpen));
    }

    #[test]
    fn a_file_is_known_by_its_extension() {
        assert!(matches!(icon("main.rs"), IconName::FileCode));
        assert!(matches!(icon("package.json"), IconName::FileBraces));
        assert!(matches!(icon("Cargo.toml"), IconName::FileCog));
        assert!(matches!(icon("README.md"), IconName::FileText));
        assert!(matches!(icon("logo.PNG"), IconName::FileImage));
    }

    #[test]
    fn a_dotfile_is_known_by_what_follows_the_dot() {
        assert!(matches!(icon(".gitignore"), IconName::FileCog));
    }

    #[test]
    fn anything_else_is_a_plain_file() {
        assert!(matches!(icon("Makefile"), IconName::File));
        assert!(matches!(icon("notes.xyz"), IconName::File));
    }

    #[test]
    fn the_app_embeds_the_tree_icons_and_keeps_the_kits_own() {
        for name in [IconName::FolderOpen, IconName::FileCode, IconName::Close] {
            let path = name.path();
            assert!(
                AppAssets.load(&path).unwrap().is_some(),
                "{path} is not embedded"
            );
        }
    }
}
