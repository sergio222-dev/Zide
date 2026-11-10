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
            ("<C-a>", Key::Ctrl('a')),
            ("<C-x>", Key::Ctrl('x')),
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

#[test]
fn visual_mode_selects_with_motions_and_operates() {
    assert_eq!(run("hello world", "vlld").0, "lo world");
    assert_eq!(
        run("hello world", "vey$p").0,
        "hellohello  worldo".replace("hellohello  worldo", "hello worldhello")
    );
    assert_eq!(run("hello world", "vwy$p").0, "hello worldhello w");
    assert_eq!(run("ab cd", "vex").0, " cd");
    assert_eq!(run("hello world", "vecX<Esc>").0, "X world");
}

#[test]
fn visual_line_mode_works_on_whole_lines() {
    assert_eq!(run("a\nb\nc\n", "Vjd").0, "c\n");
    assert_eq!(run("a\nb\nc\n", "Vjyjp").0, "a\nb\na\nb\nc\n");
    assert_eq!(run("a\nb\nc\n", "Vd").0, "b\nc\n");
    assert_eq!(run("a\nb\nc\n", "jVcx<Esc>").0, "a\nx\nc\n");
}

#[test]
fn visual_o_swaps_the_ends_and_escape_leaves() {
    assert_eq!(run_at("abcdef", 2, "vlohd").0, "aef");
    let mut buffer = Buffer::new("abc");
    let mut vim = Vim::default();
    vim.handle(&mut buffer, Key::Char('v'));
    assert_eq!(vim.mode(), Mode::Visual);
    assert_eq!(vim.selection(&buffer), Some((0..1, false)));
    vim.handle(&mut buffer, Key::Esc);
    assert_eq!(vim.mode(), Mode::Normal);
    assert_eq!(vim.selection(&buffer), None);
}

#[test]
fn v_twice_leaves_visual_mode() {
    let mut buffer = Buffer::new("abc");
    let mut vim = Vim::default();
    vim.handle(&mut buffer, Key::Char('v'));
    vim.handle(&mut buffer, Key::Char('v'));
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn word_objects() {
    assert_eq!(run_at("foo bar baz", 5, "diw").0, "foo  baz");
    assert_eq!(run_at("foo bar baz", 5, "daw").0, "foo baz");
    assert_eq!(run_at("foo bar", 5, "daw").0, "foo");
    assert_eq!(run_at("foo bar baz", 5, "ciwX<Esc>").0, "foo X baz");
    assert_eq!(run_at("foo.bar", 4, "diw").0, "foo.");
    assert_eq!(run_at("foo bar baz", 5, "viwd").0, "foo  baz");
}

#[test]
fn quote_objects() {
    assert_eq!(
        run_at(r#"say "hi there" now"#, 7, r#"ci"X<Esc>"#).0,
        r#"say "X" now"#
    );
    assert_eq!(run_at(r#"say "hi there" now"#, 7, r#"da""#).0, "say  now");
    // before the first quote: the next pair on the line
    assert_eq!(run(r#"x = "a""#, r#"di""#).0, r#"x = """#);
    assert_eq!(run_at("it's 'ok'", 6, "di'").0, "it's ''");
    assert_eq!(run_at(r#""""#, 0, r#"ci"x<Esc>"#).0, r#""x""#);
    assert_eq!(run("no quotes", r#"di""#).0, "no quotes");
}

#[test]
fn bracket_objects() {
    assert_eq!(run_at("f(a, g(b), c)", 3, "ci(X<Esc>").0, "f(X)");
    assert_eq!(run_at("f(a, g(b), c)", 8, "di(").0, "f(a, g(), c)");
    assert_eq!(run_at("f(a, g(b), c)", 3, "da(").0, "f");
    assert_eq!(run_at("f(a)", 1, "di)").0, "f()");
    assert_eq!(run_at("x[1] y", 2, "di[").0, "x[] y");
    assert_eq!(run_at("<a>", 1, "di>").0, "<>");
    assert_eq!(run_at("f(a)", 0, "di(").0, "f(a)"); // not inside brackets
}

#[test]
fn braces_over_several_lines_take_the_lines_between() {
    assert_eq!(
        run_at("fn f() {\n    a;\n    b;\n}\n", 15, "di{").0,
        "fn f() {\n}\n"
    );
    assert_eq!(run_at("{ a }", 2, "di{").0, "{}");
}

#[test]
fn dot_repeats_the_last_change() {
    assert_eq!(run("a b c d", "dw.").0, "c d");
    assert_eq!(run("a b c d", "dw..").0, "d");
    assert_eq!(run("foo bar baz", "cwX<Esc>w.").0, "X X baz");
    assert_eq!(run("abc", "x.").0, "c");
    assert_eq!(run("a\nb\nc\n", "dd.").0, "c\n");
    assert_eq!(run("a b", "ix<Esc>.").0, "xxa b");
}

#[test]
fn dot_ignores_motions_undo_and_itself() {
    assert_eq!(run("a b c d", "dwwu.").0, "b c d");
    assert_eq!(run("abcd", "xlh.").0, "cd");
    assert_eq!(run("abc", ".").0, "abc"); // nothing to repeat
}

#[test]
fn a_repeated_change_is_one_undo_step() {
    assert_eq!(run("a b c d", "dw.u").0, "b c d");
}

#[test]
fn move_to_line_keeps_the_wanted_column_and_clamps() {
    let mut buffer = Buffer::new("abcd\nx\nabcd\n");
    let mut vim = Vim::default();
    for c in "lll".chars() {
        vim.handle(&mut buffer, Key::Char(c));
    }
    vim.move_to_line(&buffer, 1);
    assert_eq!(vim.cursor(), 5); // the short line
    vim.move_to_line(&buffer, 2);
    assert_eq!(vim.cursor(), 7 + 3); // the wanted column is back
    vim.move_to_line(&buffer, 99);
    assert_eq!(vim.cursor(), 7 + 3); // clamped to the last line
}

#[test]
fn select_puts_the_range_in_visual_mode() {
    let mut buffer = Buffer::new("hello world\n");
    let mut vim = Vim::default();
    vim.select(&buffer, 6..11);
    assert_eq!(vim.mode(), Mode::Visual);
    assert_eq!(vim.selection(&buffer), Some((6..11, false)));
    vim.handle(&mut buffer, Key::Esc);
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn a_click_ends_visual_mode_and_pending_commands_and_lands_on_a_character() {
    let mut buffer = Buffer::new("one two\nthree\n");
    let mut vim = Vim::default();
    vim.handle(&mut buffer, Key::Char('v'));
    vim.handle(&mut buffer, Key::Char('l'));
    vim.handle(&mut buffer, Key::Char('2'));
    vim.click(&mut buffer, 9);
    assert_eq!(vim.mode(), Mode::Normal);
    assert_eq!(vim.cursor(), 9);
    assert!(vim.pending().is_empty());
    vim.click(&mut buffer, 7); // the end of the line is not a character in Normal mode
    assert_eq!(vim.cursor(), 6);
}

#[test]
fn a_click_in_insert_mode_keeps_inserting_and_may_sit_at_the_end_of_the_line() {
    let mut buffer = Buffer::new("one\ntwo\n");
    let mut vim = Vim::default();
    vim.handle(&mut buffer, Key::Char('i'));
    vim.handle(&mut buffer, Key::Char('X'));
    vim.click(&mut buffer, 4);
    assert_eq!(vim.mode(), Mode::Insert);
    assert_eq!(vim.cursor(), 4);
    vim.handle(&mut buffer, Key::Char('!'));
    assert_eq!(buffer.text(), "Xone!\ntwo\n");
}

#[test]
fn ctrl_a_and_ctrl_x_change_the_number_at_or_after_the_cursor() {
    assert_eq!(run("x = 9;", "<C-a>"), ("x = 10;".into(), 5));
    assert_eq!(run("x = 10;", "<C-x>").0, "x = 9;");
    assert_eq!(run_at("12 34", 3, "<C-a>").0, "12 35");
    assert_eq!(run_at("12 34", 1, "<C-a>").0, "13 34");
    assert_eq!(run("a 5", "5<C-a>").0, "a 10");
    assert_eq!(run("a 5", "7<C-x>").0, "a -2");
    assert_eq!(run("a -2", "<C-a>").0, "a -1");
    assert_eq!(run("a 007", "<C-a>").0, "a 008");
    assert_eq!(run("foo-1", "<C-a>").0, "foo-2");
}

#[test]
fn ctrl_a_without_a_number_changes_nothing_and_repeats_with_dot() {
    assert_eq!(run("abc\n1", "<C-a>").0, "abc\n1");
    assert_eq!(run("1", "<C-a>.").0, "3");
    assert_eq!(run("1", "<C-a>u").0, "1");
}

#[test]
fn an_opener_in_insert_mode_brings_its_closer_with_the_cursor_between() {
    for (typed, text) in [("(", "()"), ("[", "[]"), ("{", "{}"), ("\"", "\"\"")] {
        let (result, cursor) = run("", &format!("i{typed}"));
        assert_eq!((result.as_str(), cursor), (text, 1), "typing {typed}");
    }
}

#[test]
fn typing_the_closer_steps_over_the_one_that_came_along() {
    assert_eq!(run("", "i(a)").0, "(a)");
    assert_eq!(run("", "i(a)").1, 3);
    assert_eq!(run("", "i\"a\"b").0, "\"a\"b");
    assert_eq!(run("", "i{[(x)]}").0, "{[(x)]}");
}

#[test]
fn backspace_between_an_empty_pair_removes_both() {
    assert_eq!(run("", "i(<BS>").0, "");
    assert_eq!(run("", "i\"<BS>").0, "");
    assert_eq!(run("", "i(a<BS>").0, "()");
}

#[test]
fn an_opener_before_a_word_or_inside_a_string_stays_alone() {
    assert_eq!(run_at("word", 0, "i(").0, "(word");
    assert_eq!(run("", "i\"[").0, "\"[\"");
    assert_eq!(run("", "i\"{(").0, "\"{(\"");
}

#[test]
fn a_quote_after_a_word_character_is_not_a_pair() {
    assert_eq!(run("", "idon\"").0, "don\"");
}

#[test]
fn an_escaped_quote_does_not_close_the_string() {
    assert_eq!(run("", "i\"a\\\"[").0, "\"a\\\"[\"");
}
