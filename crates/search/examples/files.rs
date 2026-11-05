//! `cargo run --release -p zide-search --example files -- <root> <query>...`
//! Scans `root` once, then times each query.

use std::{path::Path, time::Instant};

use zide_index::FileIndex;
use zide_search::{FileQuery, search_files};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("usage: files <root> <query>...");

    let started = Instant::now();
    let paths = zide_core::scan_files(Path::new(&root)).expect("scan failed");
    let index = FileIndex::new(paths);
    eprintln!("scanned {} files in {:?}", index.len(), started.elapsed());

    for query in args {
        let parsed = FileQuery::parse(&query);
        let started = Instant::now();
        let hits = search_files(&index, &parsed, 10);
        eprintln!("{query:?}: {} shown in {:?}", hits.len(), started.elapsed());
        for hit in hits {
            println!("  {:>5}  {}", hit.score, index.paths()[hit.index]);
        }
    }
}
