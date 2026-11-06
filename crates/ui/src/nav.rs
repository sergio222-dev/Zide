//! The navigation history (IntelliJ's Back / Forward): the places the cursor left when the
//! user jumped somewhere else, so the mouse's back and forward buttons can return to them.

/// Most places kept in each direction.
const LIMIT: usize = 100;

/// A spot in a file, 0-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

impl Place {
    /// Whether both are on the same line of the same file, which is as exact as the history
    /// is: a jump from the middle of a line to its start leaves nothing to go back to.
    fn same_line(&self, other: &Place) -> bool {
        self.path == other.path && self.line == other.line
    }
}

#[derive(Default)]
pub struct NavHistory {
    back: Vec<Place>,
    forward: Vec<Place>,
}

impl NavHistory {
    /// The cursor is about to jump away from `place`: Back will return to it, and whatever
    /// Forward held no longer follows from where the user is.
    pub fn record(&mut self, place: Place) {
        self.forward.clear();
        if self.back.last().is_some_and(|last| last.same_line(&place)) {
            return;
        }
        self.back.push(place);
        if self.back.len() > LIMIT {
            self.back.remove(0);
        }
    }

    /// The place Back goes to, from `current`, which Forward then returns to.
    pub fn back(&mut self, current: Place) -> Option<Place> {
        Self::step(&mut self.back, &mut self.forward, current)
    }

    /// The place Forward goes to, from `current`, which Back then returns to.
    pub fn forward(&mut self, current: Place) -> Option<Place> {
        Self::step(&mut self.forward, &mut self.back, current)
    }

    /// Takes the nearest place of `from` that is not `current`, and leaves `current` in `to`.
    fn step(from: &mut Vec<Place>, to: &mut Vec<Place>, current: Place) -> Option<Place> {
        while let Some(place) = from.pop() {
            if place.same_line(&current) {
                continue; // the cursor has not left it: there is nothing to go back to
            }
            if !to.last().is_some_and(|last| last.same_line(&current)) {
                to.push(current);
                if to.len() > LIMIT {
                    to.remove(0);
                }
            }
            return Some(place);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, line: usize) -> Place {
        Place {
            path: path.to_string(),
            line,
            column: 0,
        }
    }

    #[test]
    fn back_returns_to_the_place_left_and_forward_to_where_it_came_from() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1));
        history.record(at("b.rs", 5)); // now at c.rs:9
        assert_eq!(history.back(at("c.rs", 9)), Some(at("b.rs", 5)));
        assert_eq!(history.back(at("b.rs", 5)), Some(at("a.rs", 1)));
        assert_eq!(history.back(at("a.rs", 1)), None);
        assert_eq!(history.forward(at("a.rs", 1)), Some(at("b.rs", 5)));
        assert_eq!(history.forward(at("b.rs", 5)), Some(at("c.rs", 9)));
        assert_eq!(history.forward(at("c.rs", 9)), None);
    }

    #[test]
    fn a_new_jump_drops_what_forward_held() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1));
        history.back(at("b.rs", 2));
        history.record(at("a.rs", 7));
        assert_eq!(history.forward(at("a.rs", 7)), None);
    }

    #[test]
    fn the_same_line_is_not_recorded_twice_in_a_row() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 3));
        history.record(Place {
            column: 8,
            ..at("a.rs", 3)
        });
        assert_eq!(history.back(at("b.rs", 0)), Some(at("a.rs", 3)));
        assert_eq!(history.back(at("a.rs", 3)), None);
    }

    #[test]
    fn back_skips_the_place_the_cursor_is_already_on() {
        let mut history = NavHistory::default();
        history.record(at("a.rs", 1));
        history.record(at("b.rs", 4));
        // came back to b.rs:4 by other means: Back goes past it
        assert_eq!(history.back(at("b.rs", 4)), Some(at("a.rs", 1)));
    }

    #[test]
    fn the_history_keeps_the_most_recent_places() {
        let mut history = NavHistory::default();
        for line in 0..LIMIT + 10 {
            history.record(at("a.rs", line));
        }
        assert_eq!(history.back.len(), LIMIT);
        assert_eq!(history.back(at("z.rs", 0)), Some(at("a.rs", LIMIT + 9)));
        assert_eq!(history.back.first(), Some(&at("a.rs", 10)));
    }
}
