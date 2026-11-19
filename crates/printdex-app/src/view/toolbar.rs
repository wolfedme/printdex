//! Toolbar (search, filter chips, sort, view, rescan), the filter menu bar
//! and the header line above the models.

use std::fmt;

use iced::widget::{Space, button, container, pick_list, row, stack, text, text_input};
use iced::{Alignment, Element, Length, Padding};
use printdex_core::{PrintableFormat, Scope, Smart, SortOrder};

use super::widgets;
use crate::app::{App, Message, TOOLBAR_HEIGHT, search_id};
use crate::filters::{self, Chip};
use crate::format::group;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::org::{CollectionEdit, Org, collection_input_id};
use crate::settings::{Settings, ViewMode};
use crate::theme::{self, Palette};

const SEARCH_WIDTH: f32 = 300.0;
const SEARCH_WIDTH_COMPACT: f32 = 220.0;

/// A sort order with its translated name, for the pick list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SortChoice(SortOrder);

impl fmt::Display for SortChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&match self.0 {
            SortOrder::Name => t("sort-name"),
            SortOrder::RecentlyAdded => t("sort-recent"),
            SortOrder::Collection => t("sort-collection"),
        })
    }
}

const SORTS: [SortChoice; 2] = [
    SortChoice(SortOrder::Name),
    SortChoice(SortOrder::RecentlyAdded),
];

/// Inside a collection its own order comes first.
const COLLECTION_SORTS: [SortChoice; 3] = [
    SortChoice(SortOrder::Collection),
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
        Chip::Material(m) => (t("filter-material"), m.material().label().to_owned()),
    }
}

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let settings = app.settings;
    let compact = app.compact();

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
    // Narrower in compact windows so the sort and view controls fit.
    .width(if compact {
        SEARCH_WIDTH_COMPACT
    } else {
        SEARCH_WIDTH
    })
    .height(theme::CONTROL_HEIGHT);

    let mut left = row![search].spacing(theme::S4).align_y(Alignment::Center);
    for chip in &app.chips {
        left = left.push(active_chip(p, *chip, app.motion.chip_scale(*chip)));
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

    let in_collection = matches!(app.search.query.scope, Scope::Collection(_));
    let sorts: &'static [SortChoice] = if in_collection {
        &COLLECTION_SORTS
    } else {
        &SORTS
    };
    let mut sort = row![].spacing(theme::S3).align_y(Alignment::Center);
    if !compact {
        sort = sort.push(
            text(t("sort-label"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    let sort = sort.push(
        pick_list(
            sorts,
            Some(SortChoice(app.search.query.sort)),
            move |SortChoice(sort)| {
                if in_collection {
                    Message::QuerySort(sort)
                } else {
                    Message::SettingsChanged(Settings { sort, ..settings })
                }
            },
        )
        .text_size(theme::SIZE_BODY)
        .padding([7.0, theme::S4])
        .style(move |_, status| p.pick(status))
        .menu_style(move |_| p.pick_menu()),
    );

    let retry = matches!(
        app.search.query.scope,
        Scope::Library(id) if app.offline.contains_key(&id)
    );
    let rescan_label = if retry {
        t("toolbar-retry")
    } else {
        t("toolbar-rescan")
    };
    let mut content = row![icons::icon(Icon::Refresh, theme::ICON, p.n.ink_2)]
        .spacing(7)
        .align_y(Alignment::Center);
    if !compact {
        content = content.push(text(rescan_label.clone()).size(theme::SIZE_BODY));
    }
    let mut rescan = button(content)
        .height(theme::CONTROL_HEIGHT)
        .padding([8.0, if compact { theme::S4 } else { theme::S5 }])
        .style(move |_, status| p.button_secondary(status));
    if !app.libraries.is_empty() {
        rescan = rescan.on_press(Message::Rescan);
    }
    let rescan: Element<'_, Message> = if compact {
        widgets::with_tip(p, rescan, rescan_label)
    } else {
        rescan.into()
    };

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

/// An active chip; `scale` is its scale-in (sizes scale, as iced 0.14
/// cannot transform a widget smaller).
fn active_chip<'a>(p: Palette, chip: Chip, scale: f32) -> Element<'a, Message> {
    let (key, value) = chip_text(chip);
    let size = theme::SIZE_CAPTION * scale;
    container(
        row![
            text(key).size(size).color(p.n.ink_3),
            text(value)
                .size(size)
                .font(theme::FONT_STRONG)
                .color(p.n.ink),
            button(icons::icon(
                Icon::Close,
                theme::ICON_CHIP * scale,
                p.n.ink_3
            ))
            .padding(2)
            .style(move |_, status| p.button_ghost(status))
            .on_press(Message::ToggleChip(chip)),
        ]
        .spacing(theme::S3 * scale)
        .align_y(Alignment::Center),
    )
    .padding([3.0 * scale, (theme::S2 + theme::S3) * scale])
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
    container(r.wrap().vertical_spacing(theme::S3))
        .padding([theme::S4, theme::S7])
        .width(Length::Fill)
        .style(move |_| p.bar())
        .into()
}

/// Scope title and how many models show; a collection also gets Rename
/// and Delete.
pub fn header(app: &App, p: Palette) -> Element<'_, Message> {
    let scope = &app.search.query.scope;
    let lists = &app.org.lists;
    let (title, in_scope) = match scope {
        Scope::All => (t("sidebar-all-models"), app.total_models()),
        Scope::Library(id) => (
            app.library(*id)
                .map(|l| l.display_name.clone())
                .unwrap_or_default(),
            app.counts.get(id).copied().unwrap_or(0),
        ),
        Scope::Collection(id) => lists
            .collections
            .iter()
            .find(|c| c.id == *id)
            .map(|c| (c.name.clone(), c.count))
            .unwrap_or_default(),
        Scope::Tag(name) => (
            t_args("header-tag", &[("tag", Arg::str(name.clone()))]),
            lists
                .tags
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(name))
                .map_or(0, |t| t.count),
        ),
        Scope::Smart(Smart::NeverPrinted) => (t("smart-never-printed"), lists.smart.never_printed),
        Scope::Smart(Smart::FailedLast) => (t("smart-failed-last"), lists.smart.failed_last),
        Scope::Smart(Smart::AddedThisWeek) => (t("smart-added-week"), lists.smart.added_this_week),
        Scope::Smart(Smart::Missing) => (t("smart-missing"), lists.smart.missing),
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

    let title: Element<'_, Message> = match (scope, &app.org.collection_edit) {
        (Scope::Collection(id), Some(CollectionEdit::Renaming(edit, name))) if edit == id => {
            text_input("", name)
                .id(collection_input_id())
                .on_input(|s| Message::Org(Org::RenameCollectionInput(s)))
                .on_submit(Message::Org(Org::CommitRenameCollection))
                .size(theme::SIZE_TITLE)
                .font(theme::FONT_BOLD)
                .width(320)
                .padding([2.0, theme::S4])
                .style(move |_, status| p.search_field(status))
                .into()
        }
        _ => text(title)
            .size(theme::SIZE_TITLE)
            .font(theme::FONT_BOLD)
            .color(p.n.ink)
            .wrapping(text::Wrapping::None)
            .into(),
    };
    let mut r = row![
        title,
        text(caption).size(theme::SIZE_CAPTION).color(p.n.ink_2),
    ]
    .spacing(theme::S5)
    .align_y(Alignment::Center);

    if let Scope::Collection(id) = scope {
        let id = *id;
        r = r.push(Space::new().width(Length::Fill));
        match &app.org.collection_edit {
            Some(CollectionEdit::ConfirmDelete(c)) if *c == id => {
                r = r.push(
                    text(t("collection-delete-confirm"))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2),
                );
                r = r.push(
                    button(text(t("action-cancel")).size(theme::SIZE_CAPTION))
                        .padding([4.0, theme::S4])
                        .style(move |_, status| p.button_secondary(status))
                        .on_press(Message::Org(Org::Cancel)),
                );
                r = r.push(
                    button(text(t("collection-delete")).size(theme::SIZE_CAPTION))
                        .padding([4.0, theme::S4])
                        .style(move |_, status| p.button_danger(status))
                        .on_press(Message::Org(Org::ConfirmDeleteCollection(id))),
                );
            }
            Some(CollectionEdit::Renaming(..)) => {}
            _ => {
                r = r.push(widgets::ghost_icon(
                    p,
                    Icon::Rename,
                    t("collection-rename"),
                    Message::Org(Org::StartRenameCollection(id)),
                ));
                r = r.push(widgets::ghost_icon(
                    p,
                    Icon::Remove,
                    t("collection-delete"),
                    Message::Org(Org::AskDeleteCollection(id)),
                ));
            }
        }
    }

    let mut col = iced::widget::column![r].spacing(theme::S4);
    if let Some(notice) = &app.org.notice {
        col = col.push(
            container(
                row![
                    icons::icon(Icon::Info, theme::ICON, p.n.ink_2),
                    text(notice.clone())
                        .size(theme::SIZE_CAPTION)
                        .width(Length::Fill),
                    button(icons::icon(Icon::Close, theme::ICON_DENSE, p.n.ink_3))
                        .padding(2)
                        .style(move |_, status| p.button_ghost(status))
                        .on_press(Message::Org(Org::DismissNotice)),
                ]
                .spacing(theme::S4)
                .align_y(Alignment::Center),
            )
            .padding([theme::S3, theme::S5])
            .style(move |_| p.well()),
        );
    }
    container(col)
        .padding(Padding {
            top: theme::S7,
            bottom: 0.0,
            left: theme::S7,
            right: theme::S7,
        })
        .into()
}
