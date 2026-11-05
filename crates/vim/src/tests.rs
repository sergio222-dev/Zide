use zide_text::Buffer;

use super::*;

/// Types `keys` into `text` with the cursor at `at`; `<Esc>`, `<CR>`, `<BS>` and `<C-r>` are
/// special keys. Returns the text and the cursor.
fn run_at(text: &str, at: usize, keys: &str) -> (String, usize) {
    let mut buffer = Buffer::new(text);
    let mut vim = Vim::default();
    vim.set_cursor(&buffer, at);
    let mut rest = keys;
    while let Some(c) = rest.chars().next() {
        let key = [
            ("<Esc>", Key::Esc),
            ("<CR>", Key::Enter),
            ("<BS>", Key::Backspace),
            ("<C-r>", Key::Ctrl('r')),
        ]
        .iter()
        .find(|(name, _)| rest.starts_with(name))
        .map(|&(name, key)| {
            rest = &rest[name.len()..];
            key
        })
        .unwrap_or_else(|| {
            rest = &rest[c.len_utf8()..];
            Key::Char(c)
        });
        vim.handle(&mut buffer, key);
    }
    (buffer.text(), vim.cursor())
}

fn run(text: &str, keys: &str) -> (String, usize) {
    run_at(text, 0, keys)
}

#[test]
fn hjkl_move_and_stay_inside_the_text() {
    assert_eq!(run("abc\nde\n", "l").1, 1);
    assert_eq!(run("abc\nde\n", "lllll").1, 2); // stops on the last char
    assert_eq!(run("abc\nde\n", "h").1, 0);
    assert_eq!(run("abc\nde\n", "j").1, 4);
    assert_eq!(run("abc\nde\n", "jj").1, 4); // no line below
    assert_eq!(run("abc\nde\n", "llj").1, 5); // clamps to the shorter line
}

#[test]
fn j_and_k_remember_the_wanted_column() {
    assert_eq!(run("abcd\nx\nabcd\n", "lllljj").1, 7 + 3);
}

#[test]
fn counts_repeat_motions() {
    assert_eq!(run("abcdef", "3l").1, 3);
    assert_eq!(run("a\nb\nc\nd\n", "2j").1, 4);
    assert_eq!(run("one two three four", "2w").1, 8);
}

#[test]
fn line_motions() {
    assert_eq!(run_at("  abc def", 6, "0").1, 0);
    assert_eq!(run_at("  abc def", 6, "^").1, 2);
    assert_eq!(run("  abc def", "$").1, 8);
    assert_eq!(run("a\nb\nc\n", "G").1, 4);
    assert_eq!(run("a\nb\nc\n", "Ggg").1, 0);
    assert_eq!(run("a\nb\nc\n", "2G").1, 2);
}

#[test]
fn word_motions() {
    assert_eq!(run("foo.bar baz", "w").1, 3);
    assert_eq!(run("foo.bar baz", "ww").1, 4);
    assert_eq!(run("foo.bar baz", "www").1, 8);
    assert_eq!(run("foo bar", "e").1, 2);
    assert_eq!(run_at("foo bar", 6, "b").1, 4);
    assert_eq!(run_at("foo bar", 4, "b").1, 0);
    assert_eq!(run("a\n\nb", "w").1, 2); // an empty line is a word
}

#[test]
fn insert_types_and_escape_steps_back() {
    assert_eq!(run("", "ihello<Esc>"), ("hello".into(), 4));
    assert_eq!(run("ab", "Axy<Esc>"), ("abxy".into(), 3));
    assert_eq!(run("  ab", "Ix<Esc>"), ("  xab".into(), 2));
    assert_eq!(run("ab", "ax<Esc>"), ("axb".into(), 1));
    assert_eq!(run("ab", "ix<CR>y<BS><BS><Esc>"), ("xab".into(), 0));
}

#[test]
fn o_opens_lines() {
    assert_eq!(run("a\nb\n", "ox<Esc>").0, "a\nx\nb\n");
    assert_eq!(run("a\nb\n", "Ox<Esc>").0, "x\na\nb\n");
}

#[test]
fn x_deletes_characters_without_crossing_the_line() {
    assert_eq!(run("abc\nd", "x").0, "bc\nd");
    assert_eq!(run("abc\nd", "5x").0, "\nd");
    assert_eq!(run("abc", "$x").1, 1);
}

#[test]
fn delete_with_motions() {
    assert_eq!(run("foo bar baz", "dw").0, "bar baz");
    assert_eq!(run("foo bar baz", "2dw").0, "baz");
    assert_eq!(run_at("foo bar baz", 4, "d$").0, "foo ");
    assert_eq!(run_at("foo bar baz", 4, "d0").0, "bar baz");
    assert_eq!(run("foo bar", "de").0, " bar");
    assert_eq!(run("abc", "dl").0, "bc");
}

#[test]
fn dw_on_the_last_word_keeps_the_line_break() {
    assert_eq!(run_at("foo bar\nbaz", 4, "dw").0, "foo \nbaz");
}

#[test]
fn dd_removes_whole_lines() {
    assert_eq!(run("a\nb\nc\n", "dd").0, "b\nc\n");
    assert_eq!(run("a\nb\nc\n", "2dd").0, "c\n");
    assert_eq!(run_at("a\nb\nc\n", 4, "dd"), ("a\nb\n".into(), 2));
    assert_eq!(run("a\nb", "jdd").0, "a");
    assert_eq!(run("only", "dd").0, "");
    assert_eq!(run("a\nb\nc\n", "dj").0, "c\n");
    assert_eq!(run_at("a\nb\nc\n", 2, "dk").0, "c\n");
    assert_eq!(run("a\nb\nc\n", "dG").0, "");
}

#[test]
fn change_enters_insert_mode() {
    assert_eq!(run("foo bar", "cwxy<Esc>").0, "xy bar");
    assert_eq!(run("foo bar", "ccx<Esc>").0, "x");
    assert_eq!(run("a\nb\n", "ccx<Esc>").0, "x\nb\n");
    assert_eq!(run("foo bar", "c$x<Esc>").0, "x");
}

#[test]
fn yank_and_paste() {
    assert_eq!(run("a\nb\n", "yyp").0, "a\na\nb\n");
    assert_eq!(run("a\nb\n", "yyP").0, "a\na\nb\n");
    assert_eq!(run("a\nb", "jyyp").0, "a\nb\nb");
    assert_eq!(run("a\nb\n", "ddp").0, "b\na\n");
    assert_eq!(run("foo bar", "ywP").0, "foo foo bar");
    assert_eq!(run("abc", "xp").0, "bac");
    assert_eq!(run("abc", "yl3p").0, "aaaabc");
}

#[test]
fn undo_and_redo_work_per_command() {
    assert_eq!(run("a b c", "dwdwu").0, "b c");
    assert_eq!(run("a b c", "dwdwuu").0, "a b c");
    assert_eq!(run("a b c", "dwdwuu<C-r>").0, "b c");
    assert_eq!(run("abc", "ixy<Esc>u").0, "abc"); // one typing run is one undo
}

#[test]
fn escape_cancels_pending_commands() {
    assert_eq!(run("abc", "d<Esc>x").0, "bc");
    assert_eq!(run("abc", "2<Esc>x").0, "bc");
}

#[test]
fn pending_shows_what_was_typed() {
    let mut buffer = Buffer::new("abc");
    let mut vim = Vim::default();
    for c in "2d".chars() {
        vim.handle(&mut buffer, Key::Char(c));
    }
    assert_eq!(vim.pending(), "d");
    vim.handle(&mut buffer, Key::Esc);
    assert_eq!(vim.pending(), "");
}
