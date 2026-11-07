use super::*;

fn tree(files: &[&str], dirs: &[&str]) -> Tree {
    let mut tree = Tree::new();
    let files: Vec<String> = files.iter().map(|s| s.to_string()).collect();
    let dirs: Vec<String> = dirs.iter().map(|s| s.to_string()).collect();
    tree.set_entries(&files, &dirs);
    tree
}

fn names(tree: &Tree) -> Vec<String> {
    tree.rows()
        .iter()
        .map(|r| format!("{}{}", " ".repeat(r.depth), r.name))
        .collect()
}

#[test]
fn folders_come_first_and_names_sort_naturally_ignoring_case() {
    let t = tree(
        &["b.txt", "A.txt", "file10", "file2", "src/main.rs"],
        &["src", "Zed"],
    );
    assert_eq!(
        names(&t),
        ["src", "Zed", "A.txt", "b.txt", "file2", "file10"]
    );
}

#[test]
fn natural_compare_reads_numbers_as_numbers() {
    assert_eq!(natural_compare("a2", "a10"), Ordering::Less);
    assert_eq!(natural_compare("a02", "a2"), Ordering::Equal);
    assert_eq!(natural_compare("B", "a"), Ordering::Greater);
    assert_eq!(natural_compare("a", "ab"), Ordering::Less);
}

#[test]
fn folders_start_collapsed_and_enter_expands_them() {
    let mut t = tree(&["src/a.rs", "src/b.rs"], &["src"]);
    assert_eq!(names(&t), ["src"]);
    assert_eq!(t.activate(), Outcome::None);
    assert_eq!(names(&t), ["src", " a.rs", " b.rs"]);
    t.activate();
    assert_eq!(names(&t), ["src"]);
}

#[test]
fn expanding_a_file_opens_it_like_enter() {
    let mut t = tree(&["a.rs"], &[]);
    assert_eq!(t.expand(), Outcome::Open("a.rs".into()));
    assert_eq!(t.activate(), Outcome::Open("a.rs".into()));
}

#[test]
fn expand_goes_into_an_open_folder_and_collapse_goes_back_out() {
    let mut t = tree(&["src/a.rs"], &["src"]);
    t.expand(); // opens src
    assert_eq!(t.selected(), Some(0));
    t.expand(); // goes to its first child
    assert_eq!(t.selected(), Some(1));
    t.collapse(); // a file: goes to the parent
    assert_eq!(t.selected(), Some(0));
    t.collapse(); // closes it
    assert_eq!(names(&t), ["src"]);
}

#[test]
fn empty_folders_are_listed_and_expand_to_nothing() {
    let mut t = tree(&[], &["empty"]);
    assert_eq!(t.expand(), Outcome::None);
    t.expand();
    assert_eq!(t.selected(), Some(0));
    assert_eq!(names(&t), ["empty"]);
}

#[test]
fn moving_stops_at_the_ends() {
    let mut t = tree(&["a", "b", "c"], &[]);
    t.move_by(-1);
    assert_eq!(t.selected(), Some(0));
    t.move_by(10);
    assert_eq!(t.selected(), Some(2));
    t.select_first();
    assert_eq!(t.selected(), Some(0));
    t.select_last();
    assert_eq!(t.selected(), Some(2));
}

#[test]
fn reveal_opens_the_folders_above_and_selects_the_file() {
    let mut t = tree(&["a/b/c.rs", "a/z.rs", "top.rs"], &["a", "a/b"]);
    t.reveal("a/b/c.rs");
    assert_eq!(names(&t), ["a", " b", "  c.rs", " z.rs", "top.rs"]);
    assert_eq!(t.selected(), Some(2));
}

#[test]
fn a_file_without_its_folder_listed_still_shows_under_it() {
    let mut t = tree(&["x/y/f.rs"], &[]);
    t.reveal("x/y/f.rs");
    assert_eq!(names(&t), ["x", " y", "  f.rs"]);
}

#[test]
fn a_rescan_keeps_expanded_folders_and_the_selection() {
    let mut t = tree(&["src/a.rs", "src/b.rs"], &["src"]);
    t.reveal("src/b.rs");
    t.set_entries(
        &["src/a.rs".into(), "src/b.rs".into(), "src/c.rs".into()],
        &["src".into()],
    );
    assert_eq!(names(&t), ["src", " a.rs", " b.rs", " c.rs"]);
    assert_eq!(t.selected(), Some(2));
    // the selected file disappears: the selection falls back to the first row
    t.set_entries(&["src/a.rs".into()], &["src".into()]);
    assert_eq!(t.selected(), Some(0));
}

#[test]
fn deleted_folders_are_forgotten_as_expanded() {
    let mut t = tree(&["gone/a.rs"], &["gone"]);
    t.reveal("gone/a.rs");
    t.set_entries(&[], &[]);
    t.set_entries(&["gone/a.rs".into()], &["gone".into()]);
    assert_eq!(names(&t), ["gone"]);
}

#[test]
fn click_selects_and_acts_like_enter() {
    let mut t = tree(&["d/f.rs", "x.rs"], &["d"]);
    assert_eq!(t.click(0), Outcome::None); // expands d
    assert_eq!(t.click(1), Outcome::Open("d/f.rs".into()));
    assert_eq!(t.selected(), Some(1));
}

#[test]
fn half_a_page_moves_half_the_visible_rows_and_stops_at_the_ends() {
    let files: Vec<String> = (0..30).map(|i| format!("f{i:02}")).collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let mut t = tree(&files, &[]);
    t.select(0);
    t.move_half_page(10, true);
    assert_eq!(t.selected(), Some(5));
    t.move_half_page(10, true);
    assert_eq!(t.selected(), Some(10));
    t.move_half_page(10, false);
    assert_eq!(t.selected(), Some(5));
    t.move_half_page(100, true); // half of 100 is past the last row
    assert_eq!(t.selected(), Some(29));
    t.move_half_page(100, false);
    t.move_half_page(100, false);
    assert_eq!(t.selected(), Some(0));
    t.move_half_page(0, true); // nothing laid out yet: still one row
    assert_eq!(t.selected(), Some(1));
}

#[test]
fn expand_all_opens_every_folder_and_collapse_all_closes_them_keeping_the_selection_visible() {
    let mut t = tree(&["a/b/c.rs", "a/d.rs", "z.rs"], &["a", "a/b"]);
    t.expand_all();
    assert_eq!(names(&t), ["a", " b", "  c.rs", " d.rs", "z.rs"]);
    t.select(2); // a/b/c.rs
    t.collapse_all();
    assert_eq!(names(&t), ["a", "z.rs"]);
    assert_eq!(t.selected(), Some(0));
    t.expand_all();
    assert_eq!(t.rows().len(), 5);
}
