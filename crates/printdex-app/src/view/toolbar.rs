//! Toolbar: scope title, search field, rescan.

use iced::widget::{Space, button, container, row, stack, text, text_input};
use iced::{Alignment, Element, Length, Padding};

use crate::app::{App, Message, TOOLBAR_HEIGHT, search_id};
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

const SEARCH_WIDTH: f32 = 320.0;

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let title = match app.search.scope.and_then(|id| app.library(id)) {
        Some(lib) => lib.display_name.clone(),
        None => t("sidebar-all-models"),
    };

    let field = text_input(&t("toolbar-search"), &app.search.text)
        .id(search_id())
        .on_input(Message::SearchChanged)
        .size(theme::SIZE_BODY)
        .padding(Padding {
            top: 8.0,
            bottom: 8.0,
            left: 10.0 + theme::ICON + theme::S4,
            right: 10.0,
        })
        .style(move |_, status| p.search_field(status));
    let search = stack![
        field,
        container(icons::icon(Icon::Search, theme::ICON, p.n.ink_3))
            .padding(Padding {
                left: 10.0,
                ..Padding::ZERO
            })
            .height(Length::Fill)
            .align_y(Alignment::Center),
    ]
    .width(SEARCH_WIDTH)
    .height(theme::CONTROL_HEIGHT);

    let mut rescan = button(
        row![
            icons::icon(Icon::Refresh, theme::ICON, p.n.ink_2),
            text(t("toolbar-rescan")).size(theme::SIZE_BODY),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .height(theme::CONTROL_HEIGHT)
    .padding([8.0, theme::S5])
    .style(move |_, status| p.button_secondary(status));
    if !app.libraries.is_empty() {
        rescan = rescan.on_press(Message::Rescan);
    }

    container(
        row![
            text(title)
                .size(theme::SIZE_TITLE)
                .font(theme::FONT_BOLD)
                .color(p.n.ink)
                .wrapping(text::Wrapping::None),
            Space::new().width(Length::Fill),
            search,
            rescan,
        ]
        .spacing(theme::S5)
        .align_y(Alignment::Center),
    )
    .height(TOOLBAR_HEIGHT)
    .padding([theme::S5, theme::S7])
    .align_y(Alignment::Center)
    .style(move |_| p.bar())
    .into()
}
