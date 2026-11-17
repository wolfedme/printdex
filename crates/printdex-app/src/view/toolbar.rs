//! Toolbar (search, filter chips, sort, view, rescan), the filter menu bar
//! and the header line above the models.

use std::fmt;

use iced::widget::{Space, button, container, pick_list, row, stack, text, text_input};
use iced::{Alignment, Element, Length, Padding};
use printdex_core::{PrintableFormat, SortOrder};

use super::widgets;
use crate::app::{App, Message, TOOLBAR_HEIGHT, search_id};
use crate::filters::{self, Chip};
use crate::format::group;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::settings::{Settings, ViewMode};
use crate::theme::{self, Palette};

const SEARCH_WIDTH: f32 = 300.0;

/// A sort order with its translated name, for the pick list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SortChoice(SortOrder);

impl fmt::Display for SortChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&match self.0 {
            SortOrder::Name => t("sort-name"),
            SortOrder::RecentlyAdded => t("sort-recent"),
        })
    }
}

const SORTS: [SortChoice; 2] = [
    SortChoice(SortOrder::Name),
    SortChoice(SortOrder::RecentlyAdded),
];

/// "Format" / "STL" for a chip.
fn chip_text(chip: Chip) -> (String, String) {
    match chip {
        Chip::Format(f) => (
            t("filter-format"),
            match f {
                PrintableFormat::Stl => "STL",
                PrintableFormat::ThreeMf => "3MF",
                PrintableFormat::Obj => "OBJ",
            }
            .to_owned(),
        ),
        Chip::MultiPart(false) => (t("filter-parts"), t("filter-single")),
        Chip::MultiPart(true) => (t("filter-parts"), t("filter-multi")),
    }
}

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let settings = app.settings;

    let field = text_input(&t("toolbar-search"), &app.search.query.text)
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

    let mut left = row![search].spacing(theme::S4).align_y(Alignment::Center);
    for chip in &app.chips {
        left = left.push(active_chip(p, *chip));
    }
    let menu_open = app.filter_menu;
    left = left.push(
        button(
            row![
                icons::icon(Icon::Plus, theme::ICON_DENSE, p.n.ink_2),
                text(t("filter-add")).size(theme::SIZE_CAPTION),
            ]
            .spacing(theme::S2)
            .align_y(Alignment::Center),
        )
        .padding([5.0, theme::S4])
        .style(move |_, status| {
            if menu_open {
                p.segment(true, status)
            } else {
                p.button_secondary(status)
            }
        })
        .on_press(Message::ToggleFilterMenu),
    );

    let sort = row![
        text(t("sort-label"))
            .size(theme::SIZE_CAPTION)
            .color(p.n.ink_3),
        pick_list(
            SORTS,
            Some(SortChoice(settings.sort)),
            move |SortChoice(sort)| Message::SettingsChanged(Settings { sort, ..settings }),
        )
        .text_size(theme::SIZE_BODY)
        .padding([7.0, theme::S4])
        .style(move |_, status| p.pick(status))
        .menu_style(move |_| p.pick_menu()),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);

    let retry = app
        .search
        .query
        .scope
        .is_some_and(|id| app.offline.contains_key(&id));
    let mut rescan = button(
        row![
            icons::icon(Icon::Refresh, theme::ICON, p.n.ink_2),
            text(if retry {
                t("toolbar-retry")
            } else {
                t("toolbar-rescan")
            })
            .size(theme::SIZE_BODY),
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
            left,
            Space::new().width(Length::Fill),
            sort,
            widgets::segmented(
                p,
                &[
                    (ViewMode::Grid, Some(Icon::Grid), None),
                    (ViewMode::List, Some(Icon::List), None),
                ],
                settings.view,
                move |view| Message::SettingsChanged(Settings { view, ..settings }),
            ),
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

fn active_chip<'a>(p: Palette, chip: Chip) -> Element<'a, Message> {
    let (key, value) = chip_text(chip);
    container(
        row![
            text(key).size(theme::SIZE_CAPTION).color(p.n.ink_3),
            text(value)
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_STRONG)
                .color(p.n.ink),
            button(icons::icon(Icon::Close, theme::ICON_CHIP, p.n.ink_3))
                .padding(2)
                .style(move |_, status| p.button_ghost(status))
                .on_press(Message::ToggleChip(chip)),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    )
    .padding([3.0, theme::S2 + theme::S3])
    .style(move |_| p.chip())
    .into()
}

/// The open "+ Filter" menu: every chip as a toggle.
pub fn filter_bar(app: &App, p: Palette) -> Element<'_, Message> {
    let mut r = row![].spacing(theme::S3).align_y(Alignment::Center);
    let mut last_key = String::new();
    for chip in filters::MENU {
        let (key, value) = chip_text(chip);
        if key != last_key {
            if !last_key.is_empty() {
                r = r.push(Space::new().width(theme::S6));
            }
            r = r.push(text(key.clone()).size(theme::SIZE_CAPTION).color(p.n.ink_3));
            last_key = key;
        }
        let active = app.chips.contains(&chip);
        let mut content = row![].spacing(theme::S2).align_y(Alignment::Center);
        if active {
            content = content.push(icons::icon(Icon::Check, theme::ICON_CHIP, p.t.accent_ink));
        }
        content = content.push(text(value).size(theme::SIZE_CAPTION));
        r = r.push(
            button(content)
                .padding([4.0, theme::S4])
                .style(move |_, status| {
                    if active {
                        p.segment(true, status)
                    } else {
                        p.button_secondary(status)
                    }
                })
                .on_press(Message::ToggleChip(chip)),
        );
    }
    container(r)
        .padding([theme::S4, theme::S7])
        .width(Length::Fill)
        .style(move |_| p.bar())
        .into()
}

/// Scope title and how many models show.
pub fn header(app: &App, p: Palette) -> Element<'_, Message> {
    let scope = app.search.query.scope;
    let title = match scope.and_then(|id| app.library(id)) {
        Some(lib) => lib.display_name.clone(),
        None => t("sidebar-all-models"),
    };
    let in_scope = match scope {
        Some(id) => app.counts.get(&id).copied().unwrap_or(0),
        None => app.total_models(),
    };
    let filtered = !app.search.query.text.trim().is_empty() || !app.chips.is_empty();
    let caption = if filtered {
        t_args(
            "header-match",
            &[
                ("shown", Arg::str(group(app.search.total))),
                ("total", Arg::str(group(in_scope))),
            ],
        )
    } else {
        t_args(
            "header-models",
            &[
                ("count", Arg::count(in_scope)),
                ("n", Arg::str(group(in_scope))),
            ],
        )
    };
    container(
        row![
            text(title)
                .size(theme::SIZE_TITLE)
                .font(theme::FONT_BOLD)
                .color(p.n.ink)
                .wrapping(text::Wrapping::None),
            text(caption).size(theme::SIZE_CAPTION).color(p.n.ink_2),
        ]
        .spacing(theme::S5)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: theme::S7,
        bottom: 0.0,
        left: theme::S7,
        right: theme::S7,
    })
    .into()
}
