//! A three-way merge by hand: the base of a conflicted file and the two versions that came from
//! it are split into the lines both agree on and the *changes* where a side differs from the
//! base. Each change is applied to the result, or discarded, one side at a time.

use std::ops::Range;

use similar::{Algorithm, capture_diff_slices};

/// The two sides of a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Yours,
    Theirs,
}

/// Who changed the lines of a [`Change`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Yours,
    Theirs,
    /// Both sides made the same change.
    Both,
    /// Both sides changed the same lines in different ways.
    Conflict,
}

impl Kind {
    pub fn changed_by(self, side: Which) -> bool {
        match side {
            Which::Yours => self != Kind::Theirs,
            Which::Theirs => self != Kind::Yours,
        }
    }
}

/// What was done with the version of a side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handling {
    Pending,
    Applied,
    Discarded,
}

/// Lines where at least one side differs from the base, as ranges of line numbers (0-based) of
/// the base and of each side. A side that did not change them has the base's lines there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub base: Range<usize>,
    pub yours: Range<usize>,
    pub theirs: Range<usize>,
    pub kind: Kind,
    yours_is: Handling,
    theirs_is: Handling,
    /// The sides applied, in the order they were: the result lists them one under the other.
    applied: Vec<Which>,
}

impl Change {
    pub fn handling(&self, side: Which) -> Handling {
        match side {
            Which::Yours => self.yours_is,
            Which::Theirs => self.theirs_is,
        }
    }

    fn set(&mut self, side: Which, handling: Handling) {
        match side {
            Which::Yours => self.yours_is = handling,
            Which::Theirs => self.theirs_is = handling,
        }
    }

    /// Every side that changed these lines was applied or discarded.
    pub fn resolved(&self) -> bool {
        [Which::Yours, Which::Theirs]
            .into_iter()
            .all(|side| !self.kind.changed_by(side) || self.handling(side) != Handling::Pending)
    }
}

/// A run of the file: lines nobody changed, or a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Lines of the base (and of both sides).
    Common(Range<usize>),
    Change(Change),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge {
    base: Vec<String>,
    yours: Vec<String>,
    theirs: Vec<String>,
    segments: Vec<Segment>,
}

fn lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

/// Where a side differs from the base: ranges of the base and of the side, in order.
fn edits(base: &[String], side: &[String]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut edits: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            continue;
        }
        match edits.last_mut() {
            Some((last_old, last_new))
                if last_old.end == old.start && last_new.end == new.start =>
            {
                last_old.end = old.end;
                last_new.end = new.end;
            }
            _ => edits.push((old, new)),
        }
    }
    edits
}

impl Merge {
    pub fn new(base: &str, yours: &str, theirs: &str) -> Self {
        let (base, yours, theirs) = (lines(base), lines(yours), lines(theirs));
        let (edits_yours, edits_theirs) = (edits(&base, &yours), edits(&base, &theirs));
        let (mut y, mut t) = (0, 0);
        // how far the lines of each side are from those of the base, after the changes so far
        let (mut delta_y, mut delta_t) = (0isize, 0isize);
        let mut segments = Vec::new();
        let mut at = 0; // base lines before this are in a segment already
        let shift = |index: usize, delta: isize| index.checked_add_signed(delta).unwrap_or(0);
        loop {
            let next = match (edits_yours.get(y), edits_theirs.get(t)) {
                (None, None) => break,
                (Some(a), None) => a.0.start,
                (None, Some(b)) => b.0.start,
                (Some(a), Some(b)) => a.0.start.min(b.0.start),
            };
            if next > at {
                segments.push(Segment::Common(at..next));
            }
            // the group: changes that touch each other, whichever side made them
            let start = next;
            let mut end = next;
            let (y_from, t_from) = (shift(start, delta_y), shift(start, delta_t));
            let (mut by_yours, mut by_theirs) = (false, false);
            loop {
                if let Some((base, side)) = edits_yours.get(y).filter(|e| e.0.start <= end) {
                    end = end.max(base.end);
                    delta_y = side.end as isize - base.end as isize;
                    y += 1;
                    by_yours = true;
                } else if let Some((base, side)) = edits_theirs.get(t).filter(|e| e.0.start <= end)
                {
                    end = end.max(base.end);
                    delta_t = side.end as isize - base.end as isize;
                    t += 1;
                    by_theirs = true;
                } else {
                    break;
                }
            }
            let yours_range = y_from..shift(end, delta_y);
            let theirs_range = t_from..shift(end, delta_t);
            let kind = match (by_yours, by_theirs) {
                (true, false) => Kind::Yours,
                (false, true) => Kind::Theirs,
                _ if yours[yours_range.clone()] == theirs[theirs_range.clone()] => Kind::Both,
                _ => Kind::Conflict,
            };
            segments.push(Segment::Change(Change {
                base: start..end,
                yours: yours_range,
                theirs: theirs_range,
                kind,
                yours_is: Handling::Pending,
                theirs_is: Handling::Pending,
                applied: Vec::new(),
            }));
            at = end;
        }
        if at < base.len() {
            segments.push(Segment::Common(at..base.len()));
        }
        Self {
            base,
            yours,
            theirs,
            segments,
        }
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn yours(&self) -> &[String] {
        &self.yours
    }

    pub fn theirs(&self) -> &[String] {
        &self.theirs
    }

    pub fn base(&self) -> &[String] {
        &self.base
    }

    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Change(change) => Some(change),
            Segment::Common(_) => None,
        })
    }

    fn change_mut(&mut self, index: usize) -> Option<&mut Change> {
        match self.segments.get_mut(index) {
            Some(Segment::Change(change)) => Some(change),
            _ => None,
        }
    }

    /// Puts what `side` did to the lines of the segment `index` in the result. Where both sides
    /// made the same change it counts for both.
    pub fn apply(&mut self, index: usize, side: Which) {
        let Some(change) = self.change_mut(index) else {
            return;
        };
        if !change.kind.changed_by(side) || change.handling(side) != Handling::Pending {
            return;
        }
        change.applied.push(side);
        if change.kind == Kind::Both {
            (change.yours_is, change.theirs_is) = (Handling::Applied, Handling::Applied);
        } else {
            change.set(side, Handling::Applied);
        }
    }

    /// Leaves what `side` did out of the result.
    pub fn discard(&mut self, index: usize, side: Which) {
        let Some(change) = self.change_mut(index) else {
            return;
        };
        if !change.kind.changed_by(side) || change.handling(side) != Handling::Pending {
            return;
        }
        if change.kind == Kind::Both {
            (change.yours_is, change.theirs_is) = (Handling::Discarded, Handling::Discarded);
        } else {
            change.set(side, Handling::Discarded);
        }
    }

    /// Applies every change only one side made, or both made alike, that is still pending.
    pub fn apply_non_conflicting(&mut self) {
        for index in 0..self.segments.len() {
            let Some(change) = self.change_mut(index) else {
                continue;
            };
            let side = match change.kind {
                Kind::Yours | Kind::Both => Which::Yours,
                Kind::Theirs => Which::Theirs,
                Kind::Conflict => continue,
            };
            self.apply(index, side);
        }
    }

    /// Takes one side's whole version.
    pub fn accept(&mut self, side: Which) {
        let other = match side {
            Which::Yours => Which::Theirs,
            Which::Theirs => Which::Yours,
        };
        for index in 0..self.segments.len() {
            if self.change_mut(index).is_none() {
                continue;
            }
            self.apply(index, side);
            self.discard(index, other);
        }
    }

    /// Conflicts that still need a choice.
    pub fn conflicts_left(&self) -> usize {
        self.changes()
            .filter(|change| change.kind == Kind::Conflict && !change.resolved())
            .count()
    }

    /// Changes that need no choice and are still pending.
    pub fn changes_left(&self) -> usize {
        self.changes()
            .filter(|change| change.kind != Kind::Conflict && !change.resolved())
            .count()
    }

    /// The lines of the result for a segment, as they are now.
    pub fn result_of(&self, segment: &Segment) -> Vec<&str> {
        match segment {
            Segment::Common(range) => self.base[range.clone()]
                .iter()
                .map(String::as_str)
                .collect(),
            Segment::Change(change) if change.applied.is_empty() => self.base[change.base.clone()]
                .iter()
                .map(String::as_str)
                .collect(),
            Segment::Change(change) => {
                let mut result = Vec::new();
                for side in &change.applied {
                    let (all, range) = match side {
                        Which::Yours => (&self.yours, &change.yours),
                        Which::Theirs => (&self.theirs, &change.theirs),
                    };
                    result.extend(all[range.clone()].iter().map(String::as_str));
                }
                result
            }
        }
    }

    /// The text of the result.
    pub fn result(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            for line in self.result_of(segment) {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n'); // the last line of a side had none, and something follows
                }
                text.push_str(line);
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(merge: &Merge) -> Vec<Kind> {
        merge.changes().map(|change| change.kind).collect()
    }

    #[test]
    fn changes_of_one_side_and_changes_of_both_are_told_apart() {
        let merge = Merge::new("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nX\n", "a\nb\nC\nd\nY\n");
        assert_eq!(kinds(&merge), [Kind::Yours, Kind::Theirs, Kind::Conflict]);
        assert_eq!(merge.conflicts_left(), 1);
        assert_eq!(merge.changes_left(), 2);
    }

    #[test]
    fn the_same_change_on_both_sides_is_not_a_conflict() {
        let merge = Merge::new("a\nb\n", "a\nZ\n", "a\nZ\n");
        assert_eq!(kinds(&merge), [Kind::Both]);
        assert_eq!(merge.conflicts_left(), 0);
    }

    #[test]
    fn the_result_starts_as_the_base() {
        let merge = Merge::new("A\n", "C\n", "B\n");
        assert_eq!(merge.result(), "A\n");
    }

    #[test]
    fn applying_both_sides_puts_one_under_the_other_in_the_order_applied() {
        let mut merge = Merge::new("A\n", "C\n", "B\n");
        merge.apply(0, Which::Theirs);
        merge.apply(0, Which::Yours);
        assert_eq!(merge.result(), "B\nC\n");
        assert_eq!(merge.conflicts_left(), 0);
    }

    #[test]
    fn discarding_a_side_leaves_its_lines_out_and_resolves_it() {
        let mut merge = Merge::new("A\n", "C\n", "B\n");
        merge.discard(0, Which::Yours);
        assert_eq!(merge.result(), "A\n");
        assert_eq!(merge.conflicts_left(), 1);
        merge.apply(0, Which::Theirs);
        assert_eq!(merge.result(), "B\n");
        assert_eq!(merge.conflicts_left(), 0);
        // a handled side is not handled twice
        merge.apply(0, Which::Theirs);
        assert_eq!(merge.result(), "B\n");
    }

    #[test]
    fn a_side_that_did_not_change_the_lines_has_nothing_to_apply() {
        let mut merge = Merge::new("a\nb\n", "a\nB\n", "a\nb\n");
        merge.apply(1, Which::Theirs);
        assert_eq!(merge.result(), "a\nb\n");
        merge.apply(1, Which::Yours);
        assert_eq!(merge.result(), "a\nB\n");
    }

    #[test]
    fn additions_and_removals_are_changes_too() {
        // yours adds a line after `a`, theirs removes `c`
        let mut merge = Merge::new("a\nb\nc\n", "a\nnew\nb\nc\n", "a\nb\n");
        assert_eq!(kinds(&merge), [Kind::Yours, Kind::Theirs]);
        merge.apply_non_conflicting();
        assert_eq!(merge.result(), "a\nnew\nb\n");
        assert_eq!(merge.changes_left(), 0);
    }

    #[test]
    fn both_sides_adding_at_the_same_place_conflict() {
        let mut merge = Merge::new("a\nb\n", "a\nx\nb\n", "a\ny\nb\n");
        assert_eq!(kinds(&merge), [Kind::Conflict]);
        merge.apply(1, Which::Yours);
        merge.apply(1, Which::Theirs);
        assert_eq!(merge.result(), "a\nx\ny\nb\n");
    }

    #[test]
    fn accepting_a_side_gives_its_version_of_the_file() {
        let (base, yours, theirs) = ("a\nb\nc\n", "A\nb\nc\nd\n", "a\nB\nc\n");
        let mut merge = Merge::new(base, yours, theirs);
        merge.accept(Which::Yours);
        assert_eq!(merge.result(), yours);
        let mut merge = Merge::new(base, yours, theirs);
        merge.accept(Which::Theirs);
        assert_eq!(merge.result(), theirs);
    }

    #[test]
    fn applying_everything_without_conflicts_is_the_merge_of_both() {
        let mut merge = Merge::new("a\nb\nc\nd\n", "A\nb\nc\nd\n", "a\nb\nc\nD\n");
        merge.apply_non_conflicting();
        assert_eq!(merge.result(), "A\nb\nc\nD\n");
        assert_eq!(merge.conflicts_left(), 0);
    }

    #[test]
    fn a_last_line_without_newline_does_not_glue_to_the_next_one() {
        let mut merge = Merge::new("a\n", "a\nx", "a\ny");
        merge.apply(1, Which::Yours);
        merge.apply(1, Which::Theirs);
        assert_eq!(merge.result(), "a\nx\ny");
    }

    #[test]
    fn files_without_changes_or_empty_bases_are_handled() {
        assert_eq!(Merge::new("a\n", "a\n", "a\n").changes().count(), 0);
        // both added the file: the base is empty
        let mut merge = Merge::new("", "x\n", "y\n");
        assert_eq!(kinds(&merge), [Kind::Conflict]);
        merge.apply(0, Which::Yours);
        assert_eq!(merge.result(), "x\n");
        assert_eq!(Merge::new("", "", "").result(), "");
    }
}
