//! `cargo run -p zide-lsp --example completion -- <root> <file> <line> <column> <typed>`
//! Types `typed` at that 1-based position (in memory, the file is not written) and lists what
//! the file's language server offers at the end of it, ranked as the editor would.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

use zide_lsp::{Client, CompletionTrigger, rank, server_for, utf16_column, word_start};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, file, line, column, typed] = args.as_slice() else {
        eprintln!("usage: completion <root> <file> <line> <column> <typed>");
        return;
    };
    let root = PathBuf::from(root);
    let (line, column): (usize, usize) = (line.parse().unwrap(), column.parse().unwrap());
    let (spec, language) = server_for(file).expect("no server for this file");
    let path = root.join(file);
    let original = fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let source = &mut lines[line - 1];
    let at = source
        .char_indices()
        .nth(column - 1)
        .map_or(source.len(), |(i, _)| i);
    source.insert_str(at, typed);
    let source_line = source.clone();
    let text = lines.join("\n");
    let cursor = column - 1 + typed.chars().count();

    let started = Instant::now();
    let client = Client::start(spec, &root, || {}).unwrap();
    println!("{} started in {:?}", client.label(), started.elapsed());
    println!("triggers: {:?}", client.completion_triggers());
    client.sync(&path, language, &text).unwrap();
    let trigger = match typed.chars().last() {
        Some(c) if client.completion_triggers().contains(&c.to_string()) => {
            CompletionTrigger::Character(c)
        }
        _ => CompletionTrigger::Invoked,
    };
    let character = utf16_column(&source_line, cursor);
    // after a character that opens signature help, show the signature too
    let (signature_triggers, _) = client.signature_triggers();
    if let Some(c) = typed
        .chars()
        .last()
        .filter(|c| signature_triggers.contains(&c.to_string()))
    {
        let asked = Instant::now();
        let hint = client
            .signature_help(
                &path,
                line as u32 - 1,
                character,
                Some(c),
                false,
                &AtomicBool::new(false),
            )
            .unwrap();
        println!("signature in {:?}: {hint:?}", asked.elapsed());
        if let Some(range) = hint.and_then(|h| {
            Some(
                h.label
                    .chars()
                    .skip(h.active.clone()?.start)
                    .take(h.active?.len())
                    .collect::<String>(),
            )
        }) {
            println!("active parameter: {range}");
        }
        return;
    }
    // a server still loading the project may answer nothing at first
    let started = Instant::now();
    let found = loop {
        let asked = Instant::now();
        let found = client
            .completion(
                &path,
                line as u32 - 1,
                character,
                trigger,
                &AtomicBool::new(false),
            )
            .unwrap();
        println!("{} items in {:?}", found.items.len(), asked.elapsed());
        if !found.items.is_empty() || started.elapsed() > Duration::from_secs(60) {
            break found;
        }
        thread::sleep(Duration::from_secs(2));
    };
    let word: String = source_line
        .chars()
        .skip(word_start(&source_line, cursor))
        .take(cursor - word_start(&source_line, cursor))
        .collect();
    println!("incomplete: {}, typed word: {word:?}", found.incomplete);
    for i in rank(&found.items, &word).into_iter().take(10) {
        let item = &found.items[i];
        let edit = item.edit(line as u32 - 1, &source_line, cursor);
        println!(
            "{:<24} {:<14} {:?} -> {:?} at {}:{}-{} (+{} edits)",
            format!(
                "{}{}",
                item.label,
                item.label_detail.as_deref().unwrap_or("")
            ),
            item.kind
                .map_or_else(String::new, |kind| format!("{kind:?}")),
            item.detail.as_deref().unwrap_or(""),
            edit.new_text,
            edit.range.start.line + 1,
            edit.range.start.character,
            edit.range.end.character,
            item.additional_edits().len(),
        );
    }
}
