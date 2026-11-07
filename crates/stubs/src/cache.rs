//! Stubs kept on disk between runs, so a restart only parses what changed meanwhile.
//!
//! The cache is only a source of stubs to reuse: every entry is checked against the file's
//! [`Stamp`](crate::Stamp) before use, so a stale or damaged cache costs time, never correctness.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::{FileStubs, StubStore};

const MAGIC: &[u8; 4] = b"ZSTB";
/// Bump when the serialized layout of [`FileStubs`] changes.
const FORMAT_VERSION: u32 = 2;

/// Stubs by project-relative path.
#[derive(Debug, Default)]
pub struct StubCache {
    pub(crate) files: HashMap<String, Arc<FileStubs>>,
}

#[derive(Serialize, Deserialize)]
struct OnDisk {
    files: Vec<(String, FileStubs)>,
}

impl StubCache {
    /// Where the cache of the project at `root` lives, if the platform has a cache directory.
    pub fn path_for(root: &Path) -> Option<PathBuf> {
        let key = xxh3_64(root.to_string_lossy().as_bytes());
        Some(
            dirs::cache_dir()?
                .join("zide")
                .join(format!("stubs-{key:016x}.bin")),
        )
    }

    /// The cache at `path`; `None` when it is missing, damaged or of another format.
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let rest = bytes.strip_prefix(MAGIC.as_slice())?;
        let (version, rest) = rest.split_at_checked(4)?;
        if u32::from_le_bytes(version.try_into().ok()?) != FORMAT_VERSION {
            return None;
        }
        let on_disk: OnDisk = postcard::from_bytes(rest).ok()?;
        Some(Self {
            files: on_disk
                .files
                .into_iter()
                .map(|(path, stubs)| (path, Arc::new(stubs)))
                .collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl StubStore {
    /// The stubs of this store, as a source to reuse in the next build.
    pub fn cache(&self) -> StubCache {
        StubCache {
            files: self
                .files()
                .paths()
                .iter()
                .zip(self.all_stubs())
                .filter_map(|(path, stubs)| Some((path.clone(), stubs.as_ref()?.clone())))
                .collect(),
        }
    }

    /// Writes the stubs to `path`, replacing it atomically.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let on_disk = OnDisk {
            files: self
                .files()
                .paths()
                .iter()
                .zip(self.all_stubs())
                .filter_map(|(path, stubs)| Some((path.clone(), stubs.as_deref()?.clone())))
                .collect(),
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend(FORMAT_VERSION.to_le_bytes());
        bytes.extend(postcard::to_stdvec(&on_disk).map_err(io::Error::other)?);

        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // each save has its own temporary file: two that overlap would rename each other's
        static SAVES: AtomicU64 = AtomicU64::new(0);
        let temporary = path.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            SAVES.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, path).inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use zide_index::FileIndex;

    fn project(name: &str) -> (PathBuf, Arc<FileIndex>) {
        let root = std::env::temp_dir().join(format!("zide-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.rs"), "struct A;\nfn go() {}\n").unwrap();
        fs::write(root.join("b.rs"), "use std::io::Read;\n").unwrap();
        (
            root,
            Arc::new(FileIndex::new(vec!["a.rs".into(), "b.rs".into()])),
        )
    }

    fn build(files: &Arc<FileIndex>, root: &Path, cache: Option<&StubCache>) -> StubStore {
        StubStore::build(files.clone(), root, cache, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn saves_that_overlap_do_not_fail() {
        // two indexings can finish together: they must not share the temporary file
        let (root, files) = project("overlap");
        let store = Arc::new(build(&files, &root, None));
        let path = root.join("cache/stubs.bin");
        let saves: Vec<_> = (0..8)
            .map(|_| {
                let (store, path) = (store.clone(), path.clone());
                std::thread::spawn(move || {
                    (0..20)
                        .try_for_each(|_| store.save(&path))
                        .map_err(|e| e.to_string())
                })
            })
            .collect();
        for save in saves {
            assert_eq!(save.join().unwrap(), Ok(()));
        }
        assert_eq!(StubCache::load(&path).unwrap().len(), 2);
    }

    #[test]
    fn a_saved_store_is_reused_without_parsing() {
        let (root, files) = project("roundtrip");
        let store = build(&files, &root, None);
        let path = root.join("cache/stubs.bin");
        store.save(&path).unwrap();

        let cache = StubCache::load(&path).unwrap();
        assert_eq!(cache.len(), 2);
        let again = build(&files, &root, Some(&cache));
        assert_eq!(again.reused(), 2);
        assert_eq!(again.len(), store.len());
        assert_eq!(again.file_stubs(1), store.file_stubs(1));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_changed_file_is_parsed_again_despite_the_cache() {
        let (root, files) = project("stale");
        let path = root.join("stubs.bin");
        build(&files, &root, None).save(&path).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(root.join("a.rs"), "struct A;\nfn go() {}\nfn more() {}\n").unwrap();
        let cache = StubCache::load(&path).unwrap();
        let store = build(&files, &root, Some(&cache));
        assert_eq!(store.reused(), 1);
        assert_eq!(store.len(), 3); // A, go, more: imports are not searchable
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn damaged_or_foreign_caches_load_as_nothing() {
        let (root, files) = project("damaged");
        let path = root.join("stubs.bin");
        assert!(StubCache::load(&path).is_none(), "missing");

        build(&files, &root, None).save(&path).unwrap();
        let good = fs::read(&path).unwrap();

        fs::write(&path, b"not a cache").unwrap();
        assert!(StubCache::load(&path).is_none(), "wrong magic");

        let mut other_version = good.clone();
        other_version[4] ^= 0xff;
        fs::write(&path, other_version).unwrap();
        assert!(StubCache::load(&path).is_none(), "other format version");

        fs::write(&path, &good[..good.len() / 2]).unwrap();
        assert!(StubCache::load(&path).is_none(), "truncated");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_cache_path_depends_on_the_project() {
        let a = StubCache::path_for(Path::new("/work/a"));
        let b = StubCache::path_for(Path::new("/work/b"));
        assert_ne!(a, b);
        assert_eq!(a, StubCache::path_for(Path::new("/work/a")));
    }
}
