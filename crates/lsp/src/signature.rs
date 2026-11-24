//! Signature help: the signature of the call the cursor is in, with the parameter being typed.

use std::ops::Range;

use lsp_types::{ParameterLabel, SignatureHelp};
use serde_json::Value;

use crate::text::char_column;

/// The signature to show over a call being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHint {
    /// The whole signature, as the server writes it (`slice(start: number, end?: number)`).
    pub label: String,
    /// The chars of `label` that name the parameter being typed, when the server knows it.
    pub active: Option<Range<usize>>,
    /// How many signatures the server offered (overloads); the hint shows the active one.
    pub count: usize,
}

/// A `textDocument/signatureHelp` answer; `None` when the cursor is not in a call.
pub fn parse_signature_help(result: Value) -> Result<Option<SignatureHint>, serde_json::Error> {
    if result.is_null() {
        return Ok(None);
    }
    let help: SignatureHelp = serde_json::from_value(result)?;
    let count = help.signatures.len();
    let index = help.active_signature.unwrap_or(0) as usize;
    let Some(signature) = help
        .signatures
        .into_iter()
        .nth(index.min(count.saturating_sub(1)))
    else {
        return Ok(None);
    };
    // the signature's own active parameter wins over the answer's
    let parameter = signature
        .active_parameter
        .or(help.active_parameter)
        .unwrap_or(0) as usize;
    let label = signature.label;
    let active = signature
        .parameters
        .unwrap_or_default()
        .into_iter()
        .nth(parameter)
        .and_then(|parameter| match parameter.label {
            // offsets count UTF-16 units, like the rest of the protocol
            ParameterLabel::LabelOffsets([start, end]) => {
                Some(char_column(&label, start)..char_column(&label, end))
            }
            // a name: its first appearance inside the parentheses, so `a` in `a(a, b)` is the
            // parameter and not the function
            ParameterLabel::Simple(name) => {
                let open = label.find('(').map_or(0, |at| at + 1);
                let byte = open + label[open..].find(name.as_str())?;
                let start = label[..byte].chars().count();
                Some(start..start + name.chars().count())
            }
        });
    Ok(Some(SignatureHint {
        label,
        active,
        count,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn active_text(hint: &SignatureHint) -> String {
        let range = hint.active.clone().unwrap();
        hint.label
            .chars()
            .skip(range.start)
            .take(range.len())
            .collect()
    }

    #[test]
    fn no_answer_or_no_signature_is_no_hint() {
        assert_eq!(parse_signature_help(Value::Null).unwrap(), None);
        assert_eq!(
            parse_signature_help(json!({"signatures": []})).unwrap(),
            None
        );
    }

    #[test]
    fn the_active_parameter_is_found_by_name_or_by_offsets() {
        let by_name = parse_signature_help(json!({
            "signatures": [{"label": "a(a: number, b: string)",
                            "parameters": [{"label": "a: number"}, {"label": "b: string"}]}],
            "activeParameter": 1,
        }))
        .unwrap()
        .unwrap();
        assert_eq!(active_text(&by_name), "b: string");
        // `é` is one char, one UTF-16 unit; `𝓍` is one char, two units
        let by_offsets = parse_signature_help(json!({
            "signatures": [{"label": "f(é, 𝓍, y)",
                            "parameters": [{"label": [2, 3]}, {"label": [5, 7]}, {"label": [9, 10]}],
                            "activeParameter": 2}],
            "activeParameter": 0,
        }))
        .unwrap()
        .unwrap();
        assert_eq!(active_text(&by_offsets), "y");
    }

    #[test]
    fn the_active_signature_of_several_is_shown() {
        let hint = parse_signature_help(json!({
            "signatures": [{"label": "f()"}, {"label": "f(x: u8)", "parameters": [{"label": "x: u8"}]}],
            "activeSignature": 1,
        }))
        .unwrap()
        .unwrap();
        assert_eq!((hint.label.as_str(), hint.count), ("f(x: u8)", 2));
        assert_eq!(active_text(&hint), "x: u8");
        // a call with no parameters has none to mark
        let none = parse_signature_help(json!({"signatures": [{"label": "f()"}]}))
            .unwrap()
            .unwrap();
        assert_eq!(none.active, None);
    }
}
