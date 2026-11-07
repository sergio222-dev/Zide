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

/// A picture and the number it is shown as: `Image #3`.
#[derive(Clone)]
pub(super) struct Numbered {
    number: u32,
    image: Arc<Image>,
}

impl Chat {
    /// What goes to Claude: the pasted pictures.
    pub(super) fn pictures(&self) -> Vec<Picture> {
        self.images
            .iter()
            .filter_map(|picture| {
                Some(Picture {
                    media_type: media_type(picture.image.format)?,
                    bytes: picture.image.bytes.clone(),
                })
            })
            .collect()
    }
}

/// The pictures of a sent message, under its text.
pub(super) fn sent_view(sent: &[Numbered]) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(sent.iter().map(|picture| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    img(picture.image.clone())
                        .max_w(px(360.))
                        .max_h(px(180.))
                        .object_fit(ObjectFit::Contain)
                        .rounded_md(),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme::DIM))
                        .child(format!("Image #{}", picture.number)),
                )
        }))
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
            let number = chat.next_image;
            chat.next_image += 1;
            chat.images.push(Numbered {
                number,
                image: Arc::new(image),
            });
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

    /// The chips of the pictures waiting to be sent, to draw inside the input box.
    pub(super) fn images_view(&self, cx: &mut Context<Self>) -> Option<Div> {
        let chat = self.claude.as_ref()?.active();
        if chat.images.is_empty() {
            return None;
        }
        let chips = chat.images.iter().enumerate().map(|(index, picture)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .rounded_md()
                .bg(rgb(theme::TREE_SELECTED))
                .text_sm()
                .text_color(rgb(theme::TEXT))
                .child(format!("Image #{}", picture.number))
                .child(
                    div()
                        .id(SharedString::from(format!("image-remove-{index}")))
                        .text_color(rgb(theme::MUTED))
                        .cursor_pointer()
                        .child("×")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.claude_remove_image(index, cx)),
                        ),
                )
        });
        Some(div().flex().flex_wrap().gap_1().pb_1().children(chips))
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::ImageFormat;

    use super::media_type;

    #[test]
    fn only_the_formats_claude_takes_have_a_media_type() {
        assert_eq!(media_type(ImageFormat::Png), Some("image/png"));
        assert_eq!(media_type(ImageFormat::Jpeg), Some("image/jpeg"));
        assert_eq!(media_type(ImageFormat::Gif), Some("image/gif"));
        assert_eq!(media_type(ImageFormat::Webp), Some("image/webp"));
        assert_eq!(media_type(ImageFormat::Bmp), None);
        assert_eq!(media_type(ImageFormat::Svg), None);
    }
}
