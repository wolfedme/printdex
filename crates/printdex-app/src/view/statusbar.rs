//! Status bar: counts and key hints.

use iced::widget::{Space, container, row, text};
use iced::{Alignment, Element, Length};

use crate::app::{App, Message};
use crate::format::group;
use crate::i18n::{Arg, t, t_args};
use crate::theme::{self, Palette};

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let s = &app.search;
    let models = if s.capped() {
        t_args(
            "status-models-capped",
            &[
                ("shown", Arg::str(group(s.hits.len() as u64))),
                ("total", Arg::str(group(s.total))),
            ],
        )
    } else {
        t_args(
            "status-models",
            &[
                ("count", Arg::count(s.total)),
                ("n", Arg::str(group(s.total))),
            ],
        )
    };
    let libs = app.libraries.len() as u64;
    let libraries = t_args(
        "status-libraries",
        &[("count", Arg::count(libs)), ("n", Arg::str(group(libs)))],
    );

    container(
        row![
            text(models)
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_MEDIUM),
            text("·").size(theme::SIZE_CAPTION).color(p.n.ink_3),
            text(libraries)
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_MEDIUM),
            Space::new().width(Length::Fill),
            hint(p, "/", t("status-key-search")),
            hint(p, "↑↓←→", t("status-key-move")),
            hint(p, "Enter", t("status-key-open-model")),
            hint(p, "Esc", t("status-key-clear")),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .height(theme::STATUSBAR_HEIGHT)
    .padding([0.0, theme::S7])
    .align_y(Alignment::Center)
    .style(move |_| p.bar())
    .into()
}

pub(super) fn hint<'a>(p: Palette, key: &'static str, label: String) -> Element<'a, Message> {
    row![
        container(text(key).size(theme::SIZE_DATA).font(theme::FONT_DATA))
            .padding([0.0, theme::S3])
            .style(move |_| p.chip()),
        text(label)
            .size(theme::SIZE_CAPTION)
            .font(theme::FONT_MEDIUM),
    ]
    .spacing(theme::S2)
    .align_y(Alignment::Center)
    .into()
}
