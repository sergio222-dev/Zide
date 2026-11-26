//! Conversions between editor text and what LSP speaks: file URIs and UTF-16 columns.

use std::path::{Path, PathBuf};

/// `file://` URI of an absolute path, percent-encoded.
pub fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The path of a `file://` URI, or `None` for any other scheme.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = encoded.get(i + 1..i + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

/// LSP columns count UTF-16 code units; the editor counts characters.
pub fn utf16_column(line: &str, char_column: usize) -> u32 {
    line.chars()
        .take(char_column)
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// The character column of a UTF-16 `column` of `line` (clamped to the line's end).
pub fn char_column(line: &str, utf16_column: u32) -> usize {
    let mut units = 0;
    for (index, c) in line.chars().enumerate() {
        if units >= utf16_column {
            return index;
        }
        units += c.len_utf16() as u32;
    }
    line.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_with_special_characters() {
        let path = Path::new("/home/ana/my proj/ñandú #1/a.rs");
        let uri = path_to_uri(path);
        assert_eq!(
            uri,
            "file:///home/ana/my%20proj/%C3%B1and%C3%BA%20%231/a.rs"
        );
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn only_file_uris_become_paths() {
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
        assert_eq!(uri_to_path("file:///a/%zz"), None);
    }

    #[test]
    fn columns_convert_between_chars_and_utf16() {
        let line = "a😀b"; // the emoji is two UTF-16 units
        assert_eq!(utf16_column(line, 0), 0);
        assert_eq!(utf16_column(line, 2), 3);
        assert_eq!(char_column(line, 3), 2);
        assert_eq!(char_column(line, 99), 3);
        assert_eq!(char_column("héllo", 2), 2);
    }
}
