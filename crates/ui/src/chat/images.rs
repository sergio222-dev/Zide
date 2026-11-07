//! Pictures in the message: Ctrl+V takes an image from the clipboard, a strip above the input
//! shows what will be sent, and each one can be removed first.

use std::sync::Arc;

use gpui_kit::*;
use zide_claude::Image as Picture;

use super::Chat;
use crate::{Workspace, theme};

/// The largest picture Claude takes.
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// The media type for a format Claude accepts, `None` for any other.
fn media_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::Webp => Some("image/webp"),
        _ => None,
    }
}

/// How the message appears in the history once sent.
pub(super) fn shown(text: &str, images: usize) -> String {
    match (images, text.is_empty()) {
        (0, _) => text.to_owned(),
        (1, true) => "[image]".to_owned(),
        (n, true) => format!("[{n} images]"),
        (1, false) => format!("[image]\n{text}"),
        (n, false) => format!("[{n} images]\n{text}"),
    }
}

impl Chat {
    /// What goes to Claude: the pasted pictures.
    pub(super) fn pictures(&self) -> Vec<Picture> {
        self.images
            .iter()
            .filter_map(|image| {
                Some(Picture {
                    media_type: media_type(image.format)?,
                    bytes: image.bytes.clone(),
                })
            })
            .collect()
    }
}

impl Workspace {
    /// Takes the picture on the clipboard into the active chat. `false` when there is none (or
    /// the clipboard also holds text, which the input pastes itself), so the key goes on.
    pub(super) fn claude_paste_image(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        if item.text().is_some_and(|text| !text.trim().is_empty()) {
            return false;
        }
        let image = item.into_entries().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image),
            _ => None,
        });
        let Some(image) = image else { return false };
        let Some(chats) = &mut self.claude else {
            return false;
        };
        let chat = chats.active_mut();
        if media_type(image.format).is_none() {
            chat.notice = Some("Only PNG, JPEG, GIF and WebP images can be sent.".to_owned());
        } else if image.bytes.len() > MAX_BYTES {
            chat.notice = Some("That image is over 5 MB, the most Claude takes.".to_owned());
        } else {
            chat.notice = None;
            chat.images.push(Arc::new(image));
        }
        true
    }

    fn claude_remove_image(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(chats) = &mut self.claude {
            let images = &mut chats.active_mut().images;
            if index < images.len() {
                images.remove(index);
            }
        }
        cx.notify();
    }

    /// The thumbnails waiting to be sent, to draw above the input.
    pub(super) fn images_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let chat = self.claude.as_ref()?.active();
        if chat.images.is_empty() {
            return None;
        }
        let thumbs = chat.images.iter().enumerate().map(|(index, image)| {
            div()
                .relative()
                .flex_none()
                .child(
                    img(image.clone())
                        .h(px(56.))
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme::CONTROL_BORDER)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("image-remove-{index}")))
                        .absolute()
                        .top_0()
                        .right_0()
                        .px_1()
                        .rounded_md()
                        .bg(rgb(theme::POPUP))
                        .text_color(rgb(theme::MUTED))
                        .cursor_pointer()
                        .child("×")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.claude_remove_image(index, cx)),
                        ),
                )
        });
        Some(div().flex().gap_2().py_1().children(thumbs))
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::ImageFormat;

    use super::{media_type, shown};

    #[test]
    fn only_the_formats_claude_takes_have_a_media_type() {
        assert_eq!(media_type(ImageFormat::Png), Some("image/png"));
        assert_eq!(media_type(ImageFormat::Jpeg), Some("image/jpeg"));
        assert_eq!(media_type(ImageFormat::Gif), Some("image/gif"));
        assert_eq!(media_type(ImageFormat::Webp), Some("image/webp"));
        assert_eq!(media_type(ImageFormat::Bmp), None);
        assert_eq!(media_type(ImageFormat::Svg), None);
    }

    #[test]
    fn the_history_says_how_many_pictures_went_with_the_message() {
        assert_eq!(shown("hi", 0), "hi");
        assert_eq!(shown("", 1), "[image]");
        assert_eq!(shown("", 3), "[3 images]");
        assert_eq!(shown("look", 1), "[image]\nlook");
        assert_eq!(shown("look", 2), "[2 images]\nlook");
    }
}
