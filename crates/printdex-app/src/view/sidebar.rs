//! Sidebar: libraries, collections, smart filters and tags with counts;
//! add library; scan progress.

use iced::widget::{button, column, container, progress_bar, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use printdex_core::{LibraryId, ScanPhase, Scope, Smart};

use crate::app::{App, Message, ScanState, Screen};
use crate::format::group;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::org::{NewFor, Org, SIDEBAR_TAGS, collection_input_id};
use crate::theme::{self, Palette};

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    // The scope is highlighted only while the library is on screen.
    let on_library = app.screen != Screen::Settings;
    let scope = &app.search.query.scope;
    let active = |s: &Scope| on_library && scope == s;

    let mut libraries = column![
        group_label(p, t("sidebar-libraries"), None),
        item(
            p,
            Icon::Library,
            t("sidebar-all-models"),
            None,
            Some(app.total_models()),
            active(&Scope::All),
            Scope::All,
        ),
    ]
    .spacing(theme::S1);
    for lib in &app.libraries {
        let count = app.counts.get(&lib.id).copied().unwrap_or(0);
        let offline = app
            .offline
            .get(&lib.id)
            .map(|time| t_args("sidebar-offline", &[("time", Arg::str(time.clone()))]));
        let s = Scope::Library(lib.id);
        libraries = libraries.push(item(
            p,
            if offline.is_some() {
                Icon::Offline
            } else {
                Icon::Folder
            },
            lib.display_name.clone(),
            offline,
            Some(count),
            active(&s),
            s,
        ));
    }
    let add = button(
        row![
            icons::icon(Icon::FolderPlus, theme::ICON, p.n.on_rail_2),
            text(t("sidebar-add-library")).size(theme::SIZE_BODY),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(theme::RAIL_ITEM_HEIGHT)
    .padding([5, 8])
    .style(move |_, status| p.rail_item(false, status))
    .on_press(Message::PickLibrary);
    libraries = libraries.push(add);

    // Collections, with inline create.
    let mut collections = column![group_label(
        p,
        t("sidebar-collections"),
        Some(Message::Org(Org::StartNewCollection(NewFor::Sidebar))),
    )]
    .spacing(theme::S1);
    for c in &app.org.lists.collections {
        let s = Scope::Collection(c.id);
        collections = collections.push(item(
            p,
            Icon::Collection,
            c.name.clone(),
            None,
            Some(c.count),
            active(&s),
            s,
        ));
    }
    match &app.org.new_collection {
        Some((NewFor::Sidebar, name)) => {
            collections = collections.push(
                text_input(&t("sidebar-new-collection"), name)
                    .id(collection_input_id())
                    .on_input(|s| Message::Org(Org::NewCollectionInput(s)))
                    .on_submit(Message::Org(Org::CommitNewCollection))
                    .size(theme::SIZE_BODY)
                    .padding([5.0, theme::S4])
                    .style(move |_, status| p.search_field(status)),
            );
        }
        _ if app.org.lists.collections.is_empty() => {
            collections = collections.push(
                container(
                    text(t("sidebar-collections-empty"))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.on_rail_2),
                )
                .padding([theme::S2, theme::S4]),
            );
        }
        _ => {}
    }

    // Smart filters: computed, never stored.
    let smart = &app.org.lists.smart;
    let mut smart_items = column![group_label(p, t("sidebar-smart"), None)].spacing(theme::S1);
    for (kind, icon, label, count) in [
        (
            Smart::NeverPrinted,
            Icon::NeverPrinted,
            t("smart-never-printed"),
            smart.never_printed,
        ),
        (
            Smart::FailedLast,
            Icon::Failed,
            t("smart-failed-last"),
            smart.failed_last,
        ),
        (
            Smart::AddedThisWeek,
            Icon::Recent,
            t("smart-added-week"),
            smart.added_this_week,
        ),
        (
            Smart::Missing,
            Icon::Offline,
            t("smart-missing"),
            smart.missing,
        ),
    ] {
        // Missing shows only while something is missing.
        if kind == Smart::Missing && count == 0 {
            continue;
        }
        let s = Scope::Smart(kind);
        smart_items = smart_items.push(item(p, icon, label, None, Some(count), active(&s), s));
    }

    // Tags, most used first.
    let tags = &app.org.lists.tags;
    let mut tag_items = column![group_label(p, t("sidebar-tags"), None)].spacing(theme::S1);
    let shown = if app.org.all_tags {
        tags.len()
    } else {
        tags.len().min(SIDEBAR_TAGS)
    };
    for tag in &tags[..shown] {
        let s = Scope::Tag(tag.name.clone());
        tag_items = tag_items.push(item(
            p,
            Icon::Tag,
            tag.name.clone(),
            None,
            Some(tag.count),
            active(&s),
            s,
        ));
    }
    if tags.is_empty() {
        tag_items = tag_items.push(
            container(
                text(t("sidebar-tags-empty"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.on_rail_2),
            )
            .padding([theme::S2, theme::S4]),
        );
    } else if tags.len() > SIDEBAR_TAGS {
        let (label, all) = if app.org.all_tags {
            (t("sidebar-tags-fewer"), false)
        } else {
            (
                t_args(
                    "sidebar-tags-all",
                    &[("count", Arg::str(group(tags.len() as u64)))],
                ),
                true,
            )
        };
        tag_items = tag_items.push(
            button(text(label).size(theme::SIZE_CAPTION).color(p.n.on_rail_2))
                .padding([theme::S2, theme::S4])
                .style(move |_, status| p.rail_item(false, status))
                .on_press(Message::Org(Org::ShowAllTags(all))),
        );
    }

    let groups = scrollable(
        column![libraries, collections, smart_items, tag_items]
            .spacing(theme::S6)
            .padding(iced::Padding {
                right: theme::S4,
                ..iced::Padding::ZERO
            }),
    )
    .direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::new().width(4).scroller_width(4),
    ))
    .style(move |_, status| p.scrollbar(status))
    .height(Length::Fill);

    let mut foot = column![].spacing(theme::S4);
    for (id, scan) in &app.scans {
        foot = foot.push(scan_block(app, p, *id, scan));
    }
    if let Some(line) = super::backup::status(&app.backup, p) {
        foot = foot.push(line);
    }

    let on_settings = app.screen == Screen::Settings;
    let settings = button(
        row![
            icons::icon(
                Icon::Settings,
                theme::ICON,
                if on_settings {
                    p.t.accent_ink
                } else {
                    p.n.on_rail_2
                }
            ),
            text(t("sidebar-settings"))
                .size(theme::SIZE_BODY)
                .width(Length::Fill),
            text("Ctrl ,")
                .size(theme::SIZE_DATA)
                .font(theme::FONT_DATA)
                .color(p.n.on_rail_2),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(theme::RAIL_ITEM_HEIGHT)
    .padding([5, 8])
    .style(move |_, status| p.rail_item(on_settings, status))
    .on_press(if on_settings {
        Message::Back
    } else {
        Message::OpenSettings
    });

    container(
        column![groups, foot, settings]
            .spacing(theme::S4)
            .height(Length::Fill),
    )
    .width(theme::SIDEBAR_WIDTH)
    .height(Length::Fill)
    .padding(theme::S5)
    .style(move |_| p.rail())
    .into()
}

/// A group heading, with an optional "+" action.
fn group_label<'a>(p: Palette, label: String, add: Option<Message>) -> Element<'a, Message> {
    let mut r = row![
        text(label.to_uppercase())
            .size(theme::SIZE_GROUP)
            .font(theme::FONT_STRONG)
            .color(p.n.on_rail_2)
            .width(Length::Fill)
    ]
    .align_y(Alignment::Center);
    if let Some(msg) = add {
        r = r.push(
            button(icons::icon(Icon::Plus, theme::ICON_DENSE, p.n.on_rail_2))
                .padding(theme::S1)
                .style(move |_, status| p.rail_item(false, status))
                .on_press(msg),
        );
    }
    container(r)
        .padding([theme::S2, theme::S4])
        .height(theme::RAIL_ITEM_HEIGHT)
        .align_y(iced::alignment::Vertical::Center)
        .into()
}

fn item<'a>(
    p: Palette,
    icon: Icon,
    label: String,
    detail: Option<String>,
    count: Option<u64>,
    active: bool,
    scope: Scope,
) -> Element<'a, Message> {
    let name = text(label)
        .size(theme::SIZE_BODY)
        .font(if active {
            theme::FONT_STRONG
        } else {
            theme::FONT_BODY
        })
        .color(if detail.is_some() {
            p.n.on_rail_2
        } else {
            p.n.on_rail
        })
        .wrapping(text::Wrapping::None);
    let label: Element<'a, Message> = match detail {
        Some(d) => column![
            name,
            text(d)
                .size(theme::SIZE_DATA)
                .color(p.n.on_rail_2)
                .wrapping(text::Wrapping::None),
        ]
        .into(),
        None => name.into(),
    };
    let icon_color = if active {
        p.t.accent_ink
    } else {
        p.n.on_rail_2
    };
    let mut r = row![
        icons::icon(icon, theme::ICON, icon_color),
        container(label).width(Length::Fill).clip(true),
    ]
    .spacing(7)
    .align_y(Alignment::Center);
    if let Some(count) = count {
        r = r.push(
            text(group(count))
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_MEDIUM)
                .color(p.n.on_rail_2),
        );
    }
    button(r)
        .width(Length::Fill)
        .padding([5, 8])
        .style(move |_, status| p.rail_item(active, status))
        .on_press(Message::SelectScope(scope))
        .into()
}

fn scan_block<'a>(app: &App, p: Palette, id: LibraryId, scan: &ScanState) -> Element<'a, Message> {
    let name = app
        .library(id)
        .map(|l| l.display_name.clone())
        .unwrap_or_default();
    let detail = if scan.phase == ScanPhase::Walking || scan.total == 0 {
        t("sidebar-scan-listing")
    } else {
        t_args(
            "sidebar-scan-progress",
            &[
                ("done", Arg::str(group(scan.done))),
                ("total", Arg::str(group(scan.total))),
            ],
        )
    };
    // Progress is a ratio for display only.
    #[allow(clippy::cast_precision_loss)]
    let ratio = if scan.total == 0 {
        0.0
    } else {
        scan.done as f32 / scan.total as f32
    };
    container(
        column![
            text(t_args("sidebar-scanning", &[("library", Arg::str(name))]))
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_STRONG),
            progress_bar(0.0..=1.0, ratio)
                .girth(4)
                .style(move |_| progress_bar::Style {
                    background: p.n.rule.into(),
                    bar: p.t.accent.into(),
                    border: iced::Border {
                        radius: theme::RADIUS_CHIP.into(),
                        ..iced::Border::default()
                    },
                }),
            text(detail).size(theme::SIZE_CAPTION),
        ]
        .spacing(theme::S2),
    )
    .padding([theme::S3, theme::S4])
    .width(Length::Fill)
    .style(move |_| p.scan_badge())
    .into()
}
