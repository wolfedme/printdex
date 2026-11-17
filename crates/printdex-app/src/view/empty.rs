//! Empty states: no library yet, no search results.

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::app::Message;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

const BODY_WIDTH: f32 = 360.0;

pub fn no_library<'a>(p: Palette) -> Element<'a, Message> {
    let add = button(
        row![
            icons::icon(Icon::FolderPlus, theme::ICON, p.t.on_primary),
            text(t("sidebar-add-library"))
                .size(theme::SIZE_BODY)
                .font(theme::FONT_STRONG),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .height(theme::CONTROL_HEIGHT)
    .padding([8.0, theme::S5])
    .style(move |_, status| p.button_primary(status))
    .on_press(Message::PickLibrary);

    centered(column![
        icons::icon(Icon::Library, theme::S9, p.n.ink_3),
        text(t("empty-library-title"))
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        text(t("empty-library-body"))
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(BODY_WIDTH)
            .align_x(Alignment::Center),
        add,
    ])
}

pub fn no_results<'a>(p: Palette, query: &str) -> Element<'a, Message> {
    centered(column![
        icons::icon(Icon::Search, theme::S9, p.n.ink_3),
        text(t_args(
            "empty-results-title",
            &[("query", Arg::str(query.trim()))]
        ))
        .size(theme::SIZE_SECTION)
        .font(theme::FONT_BOLD)
        .color(p.n.ink),
        text(t("empty-results-body"))
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(BODY_WIDTH)
            .align_x(Alignment::Center),
    ])
}

fn centered<'a>(content: iced::widget::Column<'a, Message>) -> Element<'a, Message> {
    container(content.spacing(theme::S5).align_x(Alignment::Center))
        .center(Length::Fill)
        .into()
}
