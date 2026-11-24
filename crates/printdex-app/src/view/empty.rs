//! Empty states: no library yet, no search results.

use super::focus::button;
use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length};
use printdex_core::{Scope, Smart};

use crate::app::{App, Message};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

const BODY_WIDTH: f32 = 360.0;

pub fn no_library<'a>(app: &App, p: Palette) -> Element<'a, Message> {
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
        shelf(),
        text(t("empty-library-title"))
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        text(t("empty-library-body"))
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(BODY_WIDTH)
            .align_x(Alignment::Center),
        if app.offers_restore() {
            row![
                add,
                button(text(t("restore-start")).size(theme::SIZE_BODY))
                    .height(theme::CONTROL_HEIGHT)
                    .padding([8.0, theme::S5])
                    .style(move |_, status| p.button_secondary(status))
                    .on_press(Message::Restore(crate::restore_ui::Rx::Start)),
            ]
            .spacing(theme::S3)
            .into()
        } else {
            Element::from(add)
        },
    ])
}

/// A sidebar view with nothing in it. Some of these are good news (no
/// failed prints waiting), so they say so in printed green.
pub fn empty_scope<'a>(p: Palette, scope: &Scope) -> Element<'a, Message> {
    let (icon, color, key) = match scope {
        Scope::Smart(Smart::FailedLast) => (Icon::Success, p.status.ok, "empty-failed-last"),
        Scope::Smart(Smart::NeverPrinted) => (Icon::Success, p.status.ok, "empty-never-printed"),
        Scope::Smart(Smart::Missing) => (Icon::Success, p.status.ok, "empty-missing"),
        Scope::Smart(Smart::AddedThisWeek) => (Icon::Recent, p.n.ink_3, "empty-added-week"),
        Scope::Collection(_) => (Icon::Collection, p.n.ink_3, "empty-collection"),
        Scope::Tag(_) => (Icon::Tag, p.n.ink_3, "empty-tag"),
        Scope::Library(_) | Scope::All => (Icon::Folder, p.n.ink_3, "empty-folder"),
    };
    centered(column![
        icons::icon(icon, theme::S9, color),
        text(t(&format!("{key}-title")))
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        text(t(&format!("{key}-body")))
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(BODY_WIDTH)
            .align_x(Alignment::Center),
    ])
}

/// Three prints waiting on the bed, one per material colour: the
/// library's signature before it holds anything.
fn shelf<'a>() -> Element<'a, Message> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(
        include_bytes!("../../assets/empty-shelf.svg").as_slice(),
    ))
    .width(SHELF_W)
    .height(SHELF_W / 2.0)
    .into()
}

const SHELF_W: f32 = 200.0;

pub fn no_results<'a>(p: Palette, query: &str, chips: bool) -> Element<'a, Message> {
    let (title, body) = if query.trim().is_empty() {
        (t("empty-filters-title"), t("empty-filters-body"))
    } else {
        (
            t_args("empty-results-title", &[("query", Arg::str(query.trim()))]),
            t("empty-results-body"),
        )
    };
    centered(column![
        icons::icon(Icon::Search, theme::S9, p.n.ink_3),
        text(title)
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        text(body)
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(BODY_WIDTH)
            .align_x(Alignment::Center),
        recovery(p, !query.trim().is_empty(), chips),
    ])
}

/// The way back from no results: clear the search (Esc), the filters, or both.
fn recovery<'a>(p: Palette, query: bool, chips: bool) -> Element<'a, Message> {
    let mut r = row![].spacing(theme::S3).align_y(Alignment::Center);
    if query {
        r = r.push(
            button(
                row![
                    text(t("empty-clear-search")).size(theme::SIZE_BODY),
                    container(text("Esc").size(theme::SIZE_DATA).font(theme::FONT_DATA))
                        .padding([0.0, theme::S3])
                        .style(move |_| p.chip()),
                ]
                .spacing(theme::S4)
                .align_y(Alignment::Center),
            )
            .height(theme::CONTROL_HEIGHT)
            .padding([7.0, theme::S5])
            .style(move |_, status| p.button_secondary(status))
            .on_press(Message::SearchChanged(String::new())),
        );
    }
    if chips {
        r = r.push(
            button(text(t("empty-clear-filters")).size(theme::SIZE_BODY))
                .height(theme::CONTROL_HEIGHT)
                .padding([8.0, theme::S5])
                .style(move |_, status| p.button_secondary(status))
                .on_press(Message::ClearChips),
        );
    }
    r.into()
}

fn centered<'a>(content: iced::widget::Column<'a, Message>) -> Element<'a, Message> {
    container(content.spacing(theme::S5).align_x(Alignment::Center))
        .center(Length::Fill)
        .into()
}
