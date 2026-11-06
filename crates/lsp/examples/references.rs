//! `cargo run -p zide-lsp --example references -- <root> <file> <line> <column>`
//! Finds the usages of the symbol at that 1-based position with the file's language server.

use std::{fs, path::PathBuf, time::Instant};

use zide_lsp::{Client, server_for, utf16_column};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, file, line, column] = args.as_slice() else {
        eprintln!("usage: references <root> <file> <line> <column>");
        return;
    };
    let root = PathBuf::from(root);
    let (line, column): (usize, usize) = (line.parse().unwrap(), column.parse().unwrap());
    let (spec, language) = server_for(file).expect("no server for this file");
    let path = root.join(file);
    let text = fs::read_to_string(&path).unwrap();
    let started = Instant::now();
    let client = Client::start(spec, &root, || {}).unwrap();
    println!("{} started in {:?}", spec.command, started.elapsed());
    client.sync(&path, language, &text).unwrap();
    let source_line = text.lines().nth(line - 1).unwrap();
    let character = utf16_column(source_line, column - 1);
    let started = Instant::now();
    let found = client
        .references(&path, line as u32 - 1, character)
        .unwrap();
    println!("{} references in {:?}", found.len(), started.elapsed());
    for location in found {
        let at = zide_lsp::uri_to_path(location.uri.as_str()).unwrap();
        let shown = at.strip_prefix(&root).unwrap_or(&at);
        println!(
            "{}:{}:{}",
            shown.display(),
            location.range.start.line + 1,
            location.range.start.character + 1 // UTF-16 units
        );
    }
}
