//! A model or part preview: the cached thumbnail when it has arrived, the
//! neutral placeholder until then.

use iced::widget::{Space, container, image, stack, text};
use iced::{ContentFit, Element, Length};
use printdex_core::{ThumbFailure, ThumbKind, ThumbTarget};

use crate::app::{App, Message};
use crate::i18n::t;
use crate::icons;
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

pub fn preview<'a>(app: &App, p: Palette, target: ThumbTarget, look: Look) -> Element<'a, Message> {
    let (w, h) = (look.width.max(0.0), look.height.max(0.0));
    let (picture, note): (Element<'a, Message>, Option<String>) = match app.thumbs.get(target) {
        Some(Thumb::Ready {
            handle,
            kind: ThumbKind::Render,
        }) => {
            let handle = handle.clone();
            (
                icons::on_bed(w, h, look.lift, look.shadow, move |mw, mh| {
                    image(handle).width(mw).height(mh).into()
                }),
                None,
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
            Some(match reason {
                ThumbFailure::TooLarge => t("preview-too-large"),
                _ => t("preview-unavailable"),
            }),
        ),
        None => (
            icons::placeholder_lifted(w, h, look.lift, look.shadow),
            Some(t("preview-rendering")),
        ),
    };
    let mut layers = stack![picture].width(w).height(h);
    if let Some(note) = note.filter(|_| look.label) {
        layers = layers.push(
            container(
                text(note)
                    .size(theme::SIZE_DATA)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_3),
            )
            .width(Length::Fill)
            .padding([0.0, theme::S2]),
        );
    } else {
        layers = layers.push(Space::new());
    }
    layers.into()
}
