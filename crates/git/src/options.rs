//! The options of a pull, a merge and a rebase: which each one offers, which of them cannot go
//! together, and the arguments a choice makes for git.

/// What the dialog runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Pull,
    Merge,
    Rebase,
}

impl Op {
    pub fn title(self) -> &'static str {
        match self {
            Self::Pull => "Pull",
            Self::Merge => "Merge",
            Self::Rebase => "Rebase",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::Pull => "git pull",
            Self::Merge => "git merge",
            Self::Rebase => "git rebase",
        }
    }

    /// The options it offers, in the order they are listed.
    pub fn options(self) -> &'static [OptionDef] {
        match self {
            Self::Pull => &PULL,
            Self::Merge => &MERGE,
            Self::Rebase => &REBASE,
        }
    }
}

/// What an option carries besides its flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    None,
    /// Text typed by the user; the placeholder says what.
    Text(&'static str),
    /// Another branch, picked from the list.
    Branch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionDef {
    /// What identifies it; the flag, except for the one that has none.
    pub id: &'static str,
    /// What git is given (empty when the value alone is the argument).
    pub flag: &'static str,
    pub help: &'static str,
    pub value: Value,
}

const fn option(flag: &'static str, help: &'static str) -> OptionDef {
    OptionDef {
        id: flag,
        flag,
        help,
        value: Value::None,
    }
}

const fn with_text(flag: &'static str, help: &'static str, placeholder: &'static str) -> OptionDef {
    OptionDef {
        id: flag,
        flag,
        help,
        value: Value::Text(placeholder),
    }
}

/// The rebase option that names a second branch: the one to rebase, instead of the current.
pub const OTHER_BRANCH: &str = "branch";

const PULL: [OptionDef; 6] = [
    option(
        "--rebase",
        "Rebase incoming changes on top of the current branch",
    ),
    option("--ff-only", "Merge only if it can be fast-forwarded"),
    option(
        "--no-ff",
        "Create a merge commit even if it can be fast-forwarded",
    ),
    option("--squash", "Create a single commit for all pulled changes"),
    option("--no-commit", "Merge, but do not commit the result"),
    option(
        "--no-verify",
        "Bypass the pre-merge and commit message hooks",
    ),
];

const MERGE: [OptionDef; 7] = [
    option(
        "--no-ff",
        "Create a merge commit even if it can be fast-forwarded",
    ),
    option("--ff-only", "Merge only if it can be fast-forwarded"),
    option("--squash", "Create a single commit for all merged changes"),
    with_text(
        "-m",
        "Specify a message for the merge commit",
        "Commit message",
    ),
    option("--no-commit", "Do not commit the merge result"),
    option(
        "--no-verify",
        "Bypass the pre-merge and commit message hooks",
    ),
    option(
        "--allow-unrelated-histories",
        "Allow merging histories that do not share a common ancestor",
    ),
];

const REBASE: [OptionDef; 6] = [
    OptionDef {
        id: OTHER_BRANCH,
        flag: "",
        help: "Select another branch to rebase",
        value: Value::Branch,
    },
    with_text(
        "--onto",
        "Specify a new base for the rebased commits",
        "Branch or hash",
    ),
    option("--rebase-merges", "Recreate commits topology"),
    option("--keep-empty", "Do not remove empty commits during rebase"),
    option("--root", "Rebase all commits in the branch"),
    option(
        "--update-refs",
        "Auto-update branches pointing to rebased commits",
    ),
];

/// The definition of the option `id` of `op`.
pub fn def(op: Op, id: &str) -> Option<&'static OptionDef> {
    op.options().iter().find(|def| def.id == id)
}

/// Whether `a` and `b` cannot be given together to `op`.
pub fn conflict(op: Op, a: &str, b: &str) -> bool {
    let pairs: &[(&str, &str)] = match op {
        Op::Pull => &[
            ("--rebase", "--ff-only"),
            ("--rebase", "--no-ff"),
            ("--rebase", "--squash"),
            ("--ff-only", "--no-ff"),
            ("--ff-only", "--squash"),
            ("--no-ff", "--squash"),
            ("--rebase", "--no-commit"),
        ],
        Op::Merge => &[
            ("--no-ff", "--ff-only"),
            ("--no-ff", "--squash"),
            ("--ff-only", "--squash"),
            ("--squash", "--no-commit"),
            ("-m", "--squash"),
            ("-m", "--no-commit"),
            ("-m", "--ff-only"),
        ],
        Op::Rebase => &[],
    };
    pairs
        .iter()
        .any(|&(x, y)| (x == a && y == b) || (x == b && y == a))
}

/// An option picked by the user, with its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub id: &'static str,
    pub flag: &'static str,
    pub value: String,
}

impl Chosen {
    pub fn of(def: &OptionDef) -> Self {
        Self {
            id: def.id,
            flag: def.flag,
            value: String::new(),
        }
    }
}

/// Whether `id` is out of reach: not picked, and at odds with one that is.
pub fn blocked(op: Op, chosen: &[Chosen], id: &str) -> bool {
    !chosen.iter().any(|c| c.id == id) && chosen.iter().any(|c| conflict(op, c.id, id))
}

/// The flags (each followed by its value) of what was picked, in the order it was picked.
fn flags(chosen: &[Chosen]) -> Vec<String> {
    let mut args = Vec::new();
    for option in chosen.iter().filter(|c| !c.flag.is_empty()) {
        args.push(option.flag.to_string());
        if !option.value.is_empty() {
            args.push(option.value.clone());
        }
    }
    args
}

fn has(chosen: &[Chosen], id: &str) -> bool {
    chosen.iter().any(|c| c.id == id)
}

/// `git pull`: the options, then where from. A pull that does not rebase merges, whatever
/// `pull.rebase` says, and never waits for a message to be typed.
pub fn pull_args(chosen: &[Chosen], remote: &str, branch: &str) -> Vec<String> {
    let mut args = flags(chosen);
    if !has(chosen, "--rebase") {
        args.extend(["--no-rebase".into(), "--no-edit".into()]);
    }
    args.extend([remote.to_string(), branch.to_string()]);
    args
}

/// `git merge`: the options, then what to merge. It never waits for a message to be typed.
pub fn merge_args(chosen: &[Chosen], branch: &str) -> Vec<String> {
    let mut args = flags(chosen);
    args.extend(["--no-edit".into(), branch.to_string()]);
    args
}

/// `git rebase`: the options, then the new base and, when picked, the branch to rebase. With
/// `--root` there is no old base: the branch is the new one, unless `--onto` says another.
pub fn rebase_args(chosen: &[Chosen], onto: &str) -> Vec<String> {
    let mut args = flags(chosen);
    if has(chosen, "--root") && !has(chosen, "--onto") {
        args.extend(["--onto".into(), onto.to_string()]);
    } else {
        args.push(onto.to_string());
    }
    if let Some(other) = chosen.iter().find(|c| c.id == OTHER_BRANCH) {
        args.push(other.value.clone());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(op: Op, ids: &[&str]) -> Vec<Chosen> {
        ids.iter()
            .map(|id| {
                let mut chosen = Chosen::of(def(op, id).unwrap());
                if matches!(chosen.id, "-m" | "--onto") {
                    chosen.value = "text".into();
                }
                if chosen.id == OTHER_BRANCH {
                    chosen.value = "feat/y".into();
                }
                chosen
            })
            .collect()
    }

    #[test]
    fn what_cannot_be_combined_is_blocked_until_the_other_is_dropped() {
        let chosen = pick(Op::Pull, &["--ff-only", "--no-commit"]);
        for id in ["--rebase", "--no-ff", "--squash"] {
            assert!(blocked(Op::Pull, &chosen, id), "{id}");
        }
        // what is picked, and what goes with it, is free
        for id in ["--ff-only", "--no-commit", "--no-verify"] {
            assert!(!blocked(Op::Pull, &chosen, id), "{id}");
        }
        // --no-verify goes with everything; --no-commit with everything but --rebase
        for id in [
            "--rebase",
            "--ff-only",
            "--no-ff",
            "--squash",
            "--no-commit",
        ] {
            let chosen = pick(Op::Pull, &[id]);
            assert!(!blocked(Op::Pull, &chosen, "--no-verify"), "{id}");
            assert_eq!(
                blocked(Op::Pull, &chosen, "--no-commit"),
                id == "--rebase",
                "{id}"
            );
        }
        let merge = pick(Op::Merge, &["--no-ff"]);
        assert!(blocked(Op::Merge, &merge, "--ff-only"));
        assert!(!blocked(Op::Merge, &merge, "-m"));
        assert!(blocked(Op::Merge, &pick(Op::Merge, &["-m"]), "--squash"));
        assert!(!blocked(
            Op::Rebase,
            &pick(Op::Rebase, &["--root"]),
            "--onto"
        ));
    }

    #[test]
    fn a_pull_merges_unless_it_rebases() {
        let merge = pull_args(&pick(Op::Pull, &["--ff-only"]), "origin", "main");
        assert_eq!(
            merge,
            ["--ff-only", "--no-rebase", "--no-edit", "origin", "main"]
        );
        let rebase = pull_args(&pick(Op::Pull, &["--rebase"]), "origin", "main");
        assert_eq!(rebase, ["--rebase", "origin", "main"]);
    }

    #[test]
    fn a_merge_carries_its_message_and_never_opens_an_editor() {
        let args = merge_args(&pick(Op::Merge, &["-m", "--no-verify"]), "feat/x");
        assert_eq!(args, ["-m", "text", "--no-verify", "--no-edit", "feat/x"]);
    }

    #[test]
    fn a_rebase_puts_the_base_before_the_branch_to_rebase() {
        let args = rebase_args(&pick(Op::Rebase, &["--onto", OTHER_BRANCH]), "main");
        assert_eq!(args, ["--onto", "text", "main", "feat/y"]);
        let root = rebase_args(&pick(Op::Rebase, &["--root", "--keep-empty"]), "main");
        assert_eq!(root, ["--root", "--keep-empty", "--onto", "main"]);
    }
}
