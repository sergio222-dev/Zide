/// In-memory list of the project's files, as `/`-separated paths relative to the project root.
#[derive(Debug, Default, Clone)]
pub struct FileIndex {
    paths: Vec<String>,
}

impl FileIndex {
    pub fn new(paths: Vec<String>) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}
