//! `cargo run --release -p zide-search --example text -- <root> <pattern> [--regex] [--case] [--word] [--mask '*.kt'] [--limit N]`
//! Scans `root`, then searches the contents and reports timings. Hits are only counted.

use std::{path::Path, sync::atomic::AtomicBool, time::Instant};

use zide_index::FileIndex;
use zide_search::{TextQuery, search_text};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().expect("usage: text <root> <pattern> [options]");
    let mut query = TextQuery {
        pattern: args.next().expect("missing pattern"),
        ..Default::default()
    };
    let mut limit = usize::MAX;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--regex" => query.regex = true,
            "--case" => query.case_sensitive = true,
            "--word" => query.whole_word = true,
            "--mask" => query.file_mask = args.next(),
            "--dir" => query.directory = args.next(),
            "--limit" => {
                limit = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .expect("bad --limit")
            }
            other => panic!("unknown option {other}"),
        }
    }

    let started = Instant::now();
    let index = FileIndex::new(zide_core::scan_files(Path::new(&root)).expect("scan failed"));
    eprintln!("scanned {} files in {:?}", index.len(), started.elapsed());

    let started = Instant::now();
    let mut first = None;
    let mut files = std::collections::HashSet::new();
    let total = search_text(
        &index,
        Path::new(&root),
        &query,
        limit,
        &AtomicBool::new(false),
        |hits| {
            first.get_or_insert_with(|| started.elapsed());
            files.extend(hits.iter().map(|h| h.file));
        },
    )
    .expect("search failed");
    eprintln!(
        "{total} hits in {} files; first batch after {:?}; done in {:?}",
        files.len(),
        first.unwrap_or_default(),
        started.elapsed()
    );
}
