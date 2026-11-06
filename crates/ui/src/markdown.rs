//! The Markdown preview: gpui-component's `TextView` does the parsing and rendering
//! (headings, lists, tables, code blocks, links, images); this module decides when it applies
//! and resolves the paths a document refers to against the project.

use std::path::{Path, PathBuf};

use gpui_kit::component::text::{MarkdownNode, markdown_ast};
pub use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::{
    App, Entity, ImageSource, ObjectFit, ParentElement, SharedString, Styled, StyledImage, Window,
    div, img, px, relative,
};

/// Whether `path` is a Markdown file, so the preview applies to it.
pub fn is_markdown(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
    })
}

/// What a link of the document points at.
#[derive(Debug, PartialEq, Eq)]
pub enum Link {
    /// A file of the project, as a path relative to the project root.
    File(String),
    /// Anything with a scheme (`https:`, `mailto:`): for the system to open.
    External,
    /// Nothing to open: a `#section` of the same document, or a path outside the project.
    Nowhere,
}

/// Where the link `url`, written in the project file `current`, leads.
pub fn resolve_link(current: &str, url: &str) -> Link {
    if has_scheme(url) {
        return Link::External;
    }
    match project_path(current, url) {
        Some(path) => Link::File(path),
        None => Link::Nowhere,
    }
}

/// The local file the image `url` of the project file `current` refers to, or `None` when it
/// is not a project path (a remote image, a `data:` URL, a path outside the project).
pub fn resolve_image(root: &Path, current: &str, url: &str) -> Option<PathBuf> {
    if has_scheme(url) {
        return None;
    }
    project_path(current, url).map(|path| root.join(path))
}

/// The `ImageSource` for `url`: the project file it names, or the URL itself when it is remote.
pub fn image_source(root: &Path, current: &str, url: &str) -> ImageSource {
    match resolve_image(root, current, url) {
        Some(path) => ImageSource::from(path),
        None => ImageSource::from(SharedString::from(url.to_string())),
    }
}

/// The rendering of `state` (the Markdown of the project file `current`), scrolling on its own
/// and selectable with the mouse. Clicking a link to a project file calls `open` with its path; other links go
/// to the system.
pub fn preview(
    state: &Entity<TextViewState>,
    root: PathBuf,
    current: String,
    open: impl Fn(String, &mut App) + Send + Sync + 'static,
) -> TextView {
    let link_current = current.clone();
    let row_root = root.clone();
    let row_current = current.clone();
    TextView::new(state)
        .selectable(true)
        .scrollable(true)
        .image_source(move |uri| image_source(&root, &current, &uri.to_string()))
        .markdown_block_parser(|node, _| {
            let markdown_ast::Node::Html(html) = node else {
                return None;
            };
            let images = image_row(&html.value)?;
            let alts = images.iter().map(|i| i.alt.as_str()).collect::<Vec<_>>();
            Some(MarkdownNode::new(IMAGE_ROW, images.clone()).text(alts.join(" ")))
        })
        .markdown_block_renderer(IMAGE_ROW, move |node, _: &mut Window, _: &mut App| {
            let images = node
                .data::<Vec<RowImage>>()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .children(images.iter().map(|image| {
                    let picture = img(image_source(&row_root, &row_current, &image.src))
                        .object_fit(ObjectFit::Contain)
                        .w_full();
                    let item = div().max_w(relative(1.)).pb_1().child(picture);
                    match image.width {
                        Some(Width::Fraction(f)) => item.w(relative(f)),
                        Some(Width::Px(p)) => item.w(px(p)),
                        None => item,
                    }
                }))
        })
        .on_link_click(
            move |url, _, _, cx| match resolve_link(&link_current, url) {
                Link::File(path) => open(path, cx),
                Link::External => cx.open_url(url),
                Link::Nowhere => {}
            },
        )
}

/// Name of the custom block a row of images is parsed into.
const IMAGE_ROW: &str = "image-row";

/// The width an `<img>` asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    /// `width="49%"`, as a fraction of the line.
    Fraction(f32),
    /// `width="300"` or `"300px"`.
    Px(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowImage {
    pub src: String,
    pub alt: String,
    pub width: Option<Width>,
}

/// The images of an HTML block made only of two or more `<img>` (inside an optional `<p>` or
/// `<div>`), like the screenshots side by side in a README. `None` for anything else, which the
/// regular renderer handles: it would stack these images one under the other.
pub fn image_row(html: &str) -> Option<Vec<RowImage>> {
    let mut images = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find(|c: char| !c.is_whitespace()) {
        rest = &rest[open..];
        let inner = rest.strip_prefix('<')?;
        let end = inner.find('>')?;
        let tag = &inner[..end];
        rest = &inner[end + 1..];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()?
            .to_ascii_lowercase();
        match name.as_str() {
            "p" | "div" | "br" => {}
            "img" => images.push(RowImage {
                src: attribute(tag, "src")?,
                alt: attribute(tag, "alt").unwrap_or_default(),
                width: attribute(tag, "width").and_then(|w| parse_width(&w)),
            }),
            _ => return None,
        }
    }
    (images.len() > 1).then_some(images)
}

fn parse_width(value: &str) -> Option<Width> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .map(|p| Width::Fraction(p / 100.0));
    }
    let pixels = value.strip_suffix("px").unwrap_or(value);
    pixels.trim().parse::<f32>().ok().map(Width::Px)
}

/// The quoted value of attribute `name` in the tag `tag`.
fn attribute(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        let mut from = 0;
        while let Some(at) = tag[from..].find(&needle) {
            let start = from + at;
            let value = start + needle.len();
            if tag[..start].ends_with(char::is_whitespace) {
                let len = tag[value..].find(quote)?;
                return Some(tag[value..value + len].to_string());
            }
            from = value;
        }
    }
    None
}

/// `scheme:` as in RFC 3986, at least two letters so `C:\` is not one.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `url` joined to the directory of `current` (or to the root when it starts with `/`), without
/// the `?query` and `#fragment`, `%20` decoded. `None` when it names no file or leaves the project.
fn project_path(current: &str, url: &str) -> Option<String> {
    let path = url.split(['#', '?']).next().unwrap_or("");
    if path.is_empty() {
        return None;
    }
    let path = percent_decode(path);
    let mut parts: Vec<&str> = match path.strip_prefix('/') {
        Some(_) => Vec::new(),
        None => {
            let mut dir: Vec<&str> = current.split('/').collect();
            dir.pop(); // the file name
            dir
        }
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extensions_are_recognised_ignoring_case() {
        assert!(is_markdown("README.md"));
        assert!(is_markdown("docs/guide.MD"));
        assert!(is_markdown("notes.markdown"));
    }

    #[test]
    fn other_files_are_not_markdown() {
        assert!(!is_markdown("main.rs"));
        assert!(!is_markdown("md"));
        assert!(!is_markdown("README"));
        assert!(!is_markdown("notes.md.bak"));
    }

    #[test]
    fn links_resolve_against_the_directory_of_the_open_file() {
        let file = |p: &str| Link::File(p.to_string());
        assert_eq!(
            resolve_link("docs/guide.md", "other.md"),
            file("docs/other.md")
        );
        assert_eq!(
            resolve_link("docs/guide.md", "./other.md"),
            file("docs/other.md")
        );
        assert_eq!(
            resolve_link("docs/guide.md", "../src/main.rs"),
            file("src/main.rs")
        );
        assert_eq!(
            resolve_link("README.md", "docs/guide.md"),
            file("docs/guide.md")
        );
        assert_eq!(
            resolve_link("docs/guide.md", "/src/main.rs"),
            file("src/main.rs")
        );
    }

    #[test]
    fn links_drop_the_fragment_and_query_and_decode_spaces() {
        let file = |p: &str| Link::File(p.to_string());
        assert_eq!(
            resolve_link("README.md", "docs/guide.md#install"),
            file("docs/guide.md")
        );
        assert_eq!(resolve_link("README.md", "a.md?x=1"), file("a.md"));
        assert_eq!(
            resolve_link("README.md", "my%20notes.md"),
            file("my notes.md")
        );
    }

    #[test]
    fn links_with_a_scheme_are_external() {
        assert_eq!(
            resolve_link("README.md", "https://example.com/a"),
            Link::External
        );
        assert_eq!(resolve_link("README.md", "mailto:a@b.c"), Link::External);
    }

    #[test]
    fn links_that_name_no_project_file_lead_nowhere() {
        assert_eq!(resolve_link("README.md", "#install"), Link::Nowhere);
        assert_eq!(resolve_link("README.md", ""), Link::Nowhere);
        assert_eq!(
            resolve_link("docs/guide.md", "../../outside.md"),
            Link::Nowhere
        );
        assert_eq!(
            resolve_link("README.md", "docs/"),
            Link::File("docs".to_string())
        );
    }

    #[test]
    fn images_resolve_to_files_under_the_root_and_remote_ones_do_not() {
        let root = Path::new("/p");
        assert_eq!(
            resolve_image(root, "docs/guide.md", "img/a.png"),
            Some(PathBuf::from("/p/docs/img/a.png"))
        );
        assert_eq!(resolve_image(root, "README.md", "https://x.io/a.png"), None);
        assert_eq!(
            resolve_image(root, "README.md", "data:image/png;base64,AAAA"),
            None
        );
        assert_eq!(resolve_image(root, "docs/a.md", "../../a.png"), None);
    }

    #[test]
    fn a_paragraph_of_images_is_a_row() {
        let html = "<p>\n  <img src=\"docs/a.png\" alt=\"First\" width=\"49%\">\n  \
                    <img src='docs/b.png' alt='Second' width='300px'>\n</p>";
        let row = image_row(html).unwrap();
        assert_eq!(
            row,
            vec![
                RowImage {
                    src: "docs/a.png".into(),
                    alt: "First".into(),
                    width: Some(Width::Fraction(0.49)),
                },
                RowImage {
                    src: "docs/b.png".into(),
                    alt: "Second".into(),
                    width: Some(Width::Px(300.0)),
                },
            ]
        );
    }

    #[test]
    fn images_without_a_width_or_alt_are_still_rows() {
        let row = image_row("<img src=\"a.png\" /><img src=\"b.png\" />").unwrap();
        assert_eq!(row.len(), 2);
        assert_eq!((row[0].width, row[0].alt.as_str()), (None, ""));
    }

    #[test]
    fn only_a_block_of_several_plain_images_is_a_row() {
        assert_eq!(
            image_row("<img src=\"a.png\">"),
            None,
            "one image is the default's"
        );
        assert_eq!(
            image_row("<p>text <img src=\"a.png\"><img src=\"b.png\"></p>"),
            None
        );
        assert_eq!(
            image_row("<a href=\"x\"><img src=\"a.png\"></a><img src=\"b.png\">"),
            None
        );
        assert_eq!(
            image_row("<img data-src=\"a.png\"><img src=\"b.png\">"),
            None
        );
        assert_eq!(image_row("<table></table>"), None);
    }
}
