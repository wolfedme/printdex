//! A model or part preview: the cached thumbnail when it has arrived, the
//! neutral placeholder until then.

use iced::widget::{Space, column, container, image, stack, text};
use iced::{ContentFit, Element, Length, Rectangle};
use printdex_core::{ThumbFailure, ThumbKind, ThumbTarget};

use crate::app::{App, Message};
use crate::i18n::t;
use crate::icons;
use crate::motion::Reveal;
use crate::theme::{self, Palette};
use crate::thumbs::Thumb;

/// How a preview is drawn.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub width: f32,
    pub height: f32,
    /// Hover lift in px, and the contact shadow's scale.
    pub lift: f32,
    pub shadow: f32,
    /// Show "Rendering preview" and failure notes.
    pub label: bool,
}

impl Look {
    pub fn still(width: f32, height: f32) -> Look {
        Look {
            width,
            height,
            lift: 0.0,
            shadow: 1.0,
            label: false,
        }
    }

    pub fn labelled(self) -> Look {
        Look {
            label: true,
            ..self
        }
    }
}

/// A rendered thumbnail at `w` × `h`, printing in or fading in.
fn printed<'a>(handle: image::Handle, reveal: Reveal, w: f32, h: f32) -> Element<'a, Message> {
    match reveal {
        Reveal::Shown => image(handle).width(w).height(h).into(),
        Reveal::Fade(alpha) => image(handle).width(w).height(h).opacity(alpha).into(),
        Reveal::Build { clip, .. } => {
            let src_h = printdex_core::THUMB_HEIGHT;
            let hidden = ((clip.clamp(0.0, 1.0) * src_h as f32).round() as u32).min(src_h);
            let shown = (src_h - hidden) as f32 / src_h as f32;
            let rest: Element<'a, Message> = if hidden == src_h {
                Space::new().into()
            } else {
                image(handle)
                    .crop(Rectangle {
                        x: 0,
                        y: hidden,
                        width: printdex_core::THUMB_WIDTH,
                        height: src_h - hidden,
                    })
                    .content_fit(ContentFit::Fill)
                    .width(w)
                    .height(h * shown)
                    .into()
            };
            column![Space::new().height(h * (1.0 - shown)), rest]
                .width(w)
                .height(h)
                .into()
        }
    }
}

pub fn preview<'a>(app: &App, p: Palette, target: ThumbTarget, look: Look) -> Element<'a, Message> {
    let (w, h) = (look.width.max(0.0), look.height.max(0.0));
    let (picture, note): (Element<'a, Message>, Option<(String, f32)>) = match app
        .thumbs
        .get(target)
    {
        Some(Thumb::Ready {
            handle,
            kind: ThumbKind::Render,
        }) => {
            let handle = handle.clone();
            let reveal = app.motion.reveal(target);
            let note = match reveal {
                Reveal::Build { label, .. } if label > 0.0 => Some((t("preview-rendering"), label)),
                _ => None,
            };
            (
                icons::on_bed(w, h, look.lift, look.shadow, move |mw, mh| {
                    printed(handle, reveal, mw, mh)
                }),
                note,
            )
        }
        Some(Thumb::Ready { handle, .. }) => (
            image(handle.clone())
                .content_fit(ContentFit::Contain)
                .width(w)
                .height(h)
                .into(),
            None,
        ),
        Some(Thumb::Failed(reason)) => (
            icons::placeholder_lifted(w, h, look.lift, look.shadow),
            Some((
                match reason {
                    ThumbFailure::TooLarge => t("preview-too-large"),
                    _ => t("preview-unavailable"),
                },
                1.0,
            )),
        ),
        None => (
            icons::placeholder_lifted(w, h, look.lift, look.shadow),
            Some((t("preview-rendering"), 1.0)),
        ),
    };
    let mut layers = stack![picture].width(w).height(h);
    if let Some((note, alpha)) = note.filter(|_| look.label) {
        layers = layers.push(
            container(
                text(note)
                    .size(theme::SIZE_DATA)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_3.scale_alpha(alpha)),
            )
            .width(Length::Fill)
            .padding([0.0, theme::S2]),
        );
    } else {
        layers = layers.push(Space::new());
    }
    layers.into()
}
