//! The settings Zide answers when a language server asks for its configuration.

use serde_json::{Value, json};

/// The settings of `section` (`workspace/configuration`); `null` leaves the server's defaults.
pub fn section(name: &str) -> Value {
    match name {
        // TypeScript 7 shows no inlay hints unless it is told to. It is also told to show the
        // name when the argument spells it (`name: command.name`), as an IDE does.
        "typescript" | "javascript" => json!({
            "inlayHints": {
                "parameterNames": {"enabled": "all", "suppressWhenArgumentMatchesName": false},
            },
        }),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_is_asked_for_parameter_name_hints_and_the_rest_keeps_its_defaults() {
        assert_eq!(
            section("typescript")["inlayHints"]["parameterNames"]["enabled"],
            "all"
        );
        assert_eq!(
            section("typescript")["inlayHints"]["parameterNames"]["suppressWhenArgumentMatchesName"],
            false
        );
        assert_eq!(section("javascript"), section("typescript"));
        assert_eq!(section("editor"), Value::Null);
        assert_eq!(section(""), Value::Null);
    }
}
