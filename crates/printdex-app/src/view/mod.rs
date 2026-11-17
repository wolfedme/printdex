//! Views: turn [`App`] state into widgets. No logic beyond layout.

mod empty;
mod grid;
mod list;
mod model;
mod sidebar;
mod statusbar;
mod toolbar;
mod widgets;

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::app::{App, Boot, Message, Screen};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::settings::ViewMode;
use crate::theme::{self, Palette};

pub fn view(app: &App) -> Element<'_, Message> {
    let p = app.palette();
    match &app.boot {
        Boot::Opening => centered(
            p,
            text(t("app-opening"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2),
        ),
        Boot::Failed(path) => centered(
            p,
            text(t_args(
                "app-open-failed",
                &[("path", Arg::str(path.display().to_string()))],
            ))
            .size(theme::SIZE_BODY)
            .color(p.status.fail),
        ),
        Boot::Ready => shell(app, p),
    }
}

fn shell(app: &App, p: Palette) -> Element<'_, Message> {
    let main = match app.screen {
        Screen::Model(_) => model::view(app, p),
        Screen::Library | Screen::Settings => library(app, p),
    };
    row![
        sidebar::view(app, p),
        container(Space::new().width(1).height(Length::Fill)).style(move |_| p.hairline()),
        main,
    ]
    .into()
}

fn library(app: &App, p: Palette) -> Element<'_, Message> {
    let mut content = column![].height(Length::Fill);
    if let Some(error) = &app.error {
        content = content.push(container(error_line(p, error)).padding([theme::S5, theme::S7]));
    }
    let no_library = app.libraries.is_empty() && app.search.loaded;
    if !no_library {
        content = content.push(toolbar::header(app, p));
    }
    let filtered = !app.search.query.text.trim().is_empty() || !app.chips.is_empty();
    content = content.push(if no_library {
        empty::no_library(p)
    } else if app.search.loaded && app.search.hits.is_empty() && filtered {
        empty::no_results(p, &app.search.query.text)
    } else if app.settings.view == ViewMode::List {
        list::view(app, p)
    } else {
        grid::view(app, p)
    });

    let mut top = column![toolbar::view(app, p), hairline(p)];
    if app.filter_menu {
        top = top.push(toolbar::filter_bar(app, p)).push(hairline(p));
    }
    column![
        top,
        container(content)
            .height(Length::Fill)
            .style(move |_| p.ground()),
        hairline(p),
        statusbar::view(app, p),
    ]
    .into()
}

fn centered<'a>(p: Palette, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .center(Length::Fill)
        .style(move |_| p.ground())
        .into()
}

fn hairline<'a>(p: Palette) -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
        .style(move |_| p.hairline())
        .into()
}

fn error_line<'a>(p: Palette, message: &str) -> Element<'a, Message> {
    container(
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(message.to_owned())
                .size(theme::SIZE_CAPTION)
                .width(Length::Fill),
            button(text("×").size(theme::SIZE_SECTION))
                .padding([0, 6])
                .style(move |_, _| p.bare())
                .on_press(Message::DismissError),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .padding([theme::S3, theme::S5])
    .style(move |_| p.error_line())
    .into()
}
