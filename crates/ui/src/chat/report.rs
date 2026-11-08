//! The answer to `/context` is one long Markdown report: a heading, then a table per `###`
//! section. Shown whole it floods the chat, so each section folds under its title.

/// A report split at its `###` headings.
#[derive(Debug, PartialEq)]
pub(super) struct Report {
    /// The title and the lines before the first section.
    pub head: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, PartialEq)]
pub(super) struct Section {
    pub title: String,
    pub body: String,
}

/// The sections of a `/context` answer, `None` for any other text.
pub(super) fn parse(text: &str) -> Option<Report> {
    if !text.starts_with("## Context Usage") {
        return None;
    }
    let mut head = String::new();
    let mut sections: Vec<Section> = Vec::new();
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("### ") {
            sections.push(Section {
                title: title.trim().to_owned(),
                body: String::new(),
            });
        } else if let Some(section) = sections.last_mut() {
            section.body.push_str(line);
            section.body.push('\n');
        } else {
            head.push_str(line);
            head.push('\n');
        }
    }
    Some(Report { head, sections })
}

/// Whether section `n` shows its table before the user touches it: only the first, the
/// usage by category; the per-tool and per-skill tables are long.
pub(super) fn starts_open(n: usize) -> bool {
    n == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_head_and_each_section() {
        let text =
            "## Context Usage\n\n**Model:** x\n\n### Usage\n\n| a |\n\n### MCP Tools\n\n| b |\n";
        let report = parse(text).unwrap();
        assert_eq!(report.head, "## Context Usage\n\n**Model:** x\n\n");
        let titles: Vec<_> = report.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Usage", "MCP Tools"]);
        assert_eq!(report.sections[1].body, "\n| b |\n");
    }

    #[test]
    fn other_text_is_not_a_report() {
        assert_eq!(parse("## Something else\n### a\n"), None);
        assert_eq!(parse("hello"), None);
    }

    #[test]
    fn only_the_first_section_starts_open() {
        assert!(starts_open(0));
        assert!(!starts_open(1));
    }
}
