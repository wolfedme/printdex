//! Sidebar: libraries with counts, add library, scan progress.

use iced::widget::{Space, button, column, container, progress_bar, row, text};
use iced::{Alignment, Element, Length};
use printdex_core::{LibraryId, ScanPhase};

use crate::app::{App, Message, ScanState};
use crate::format::group;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let scope = app.search.scope;
    let mut items = column![
        container(
            text(t("sidebar-libraries").to_uppercase())
                .size(theme::SIZE_GROUP)
                .font(theme::FONT_STRONG)
                .color(p.n.on_rail_2)
        )
        .padding([theme::S2, theme::S4]),
        item(
            p,
            Icon::Library,
            t("sidebar-all-models"),
            app.total_models(),
            scope.is_none(),
            None
        ),
    ]
    .spacing(theme::S1);

    for lib in &app.libraries {
        let count = app.counts.get(&lib.id).copied().unwrap_or(0);
        items = items.push(item(
            p,
            Icon::Folder,
            lib.display_name.clone(),
            count,
            scope == Some(lib.id),
            Some(lib.id),
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

    let mut foot = column![].spacing(theme::S4);
    for (id, scan) in &app.scans {
        foot = foot.push(scan_block(app, p, *id, scan));
    }

    container(
        column![items, add, Space::new().height(Length::Fill), foot]
            .spacing(theme::S4)
            .height(Length::Fill),
    )
    .width(theme::SIDEBAR_WIDTH)
    .height(Length::Fill)
    .padding(theme::S5)
    .style(move |_| p.rail())
    .into()
}

fn item<'a>(
    p: Palette,
    icon: Icon,
    label: String,
    count: u64,
    active: bool,
    scope: Option<LibraryId>,
) -> Element<'a, Message> {
    let icon_color = if active {
        p.t.accent_ink
    } else {
        p.n.on_rail_2
    };
    button(
        row![
            icons::icon(icon, theme::ICON, icon_color),
            text(label)
                .size(theme::SIZE_BODY)
                .font(if active {
                    theme::FONT_STRONG
                } else {
                    theme::FONT_BODY
                })
                .wrapping(text::Wrapping::None)
                .width(Length::Fill),
            text(group(count))
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_MEDIUM)
                .color(p.n.on_rail_2),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(theme::RAIL_ITEM_HEIGHT)
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
