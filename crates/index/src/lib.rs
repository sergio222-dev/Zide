/// In-memory list of the project's files, as `/`-separated paths relative to the project root.
#[derive(Debug, Default, Clone)]
pub struct FileIndex {
    paths: Vec<String>,
    /// Aligned with `paths` when not empty: the files git ignores, listed because the project
    /// shows ignored files. They are browsed and searched, but not parsed for symbols.
    ignored: Vec<bool>,
}

impl FileIndex {
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            ignored: Vec::new(),
        }
    }

    /// An index whose `ignored[i]` says whether git ignores `paths[i]`; a list of another
    /// length is taken as nothing ignored.
    pub fn with_ignored(paths: Vec<String>, ignored: Vec<bool>) -> Self {
        let ignored = if ignored.len() == paths.len() {
            ignored
        } else {
            Vec::new()
        };
        Self { paths, ignored }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Whether git ignores the `i`-th file.
    pub fn is_ignored(&self, i: usize) -> bool {
        self.ignored.get(i).copied().unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_files_are_told_by_position() {
        let index = FileIndex::with_ignored(vec!["a".into(), "b".into()], vec![false, true]);
        assert!(!index.is_ignored(0));
        assert!(index.is_ignored(1));
        assert!(!index.is_ignored(9));
        // a list that does not match the paths ignores nothing
        let index = FileIndex::with_ignored(vec!["a".into()], vec![true, true]);
        assert!(!index.is_ignored(0));
        assert!(!FileIndex::new(vec!["a".into()]).is_ignored(0));
    }
}
