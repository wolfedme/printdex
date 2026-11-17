//! List mode: one pane, a fixed header and virtualized 44px rows.

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length};
use printdex_core::{ModelHit, ThumbTarget};

use super::preview::{Look, preview};
use super::widgets::date;
use crate::app::{App, GRID_PAD_X, GRID_PAD_Y, GridViewport, Message, grid_id};
use crate::format::{group, parent_path};
use crate::i18n::t;
use crate::list::{HEADER_HEIGHT, ROW_HEIGHT, visible_rows};
use crate::theme::{self, Palette};

const THUMB_COLUMN: f32 = 64.0;
const PARTS_COLUMN: f32 = 72.0;
const ADDED_COLUMN: f32 = 112.0;

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let hits = &app.search.hits;
    let vp = app.grid_viewport();
    let visible = visible_rows(vp.offset, vp.height, hits.len());

    let mut rows = column![Space::new().height(visible.start as f32 * ROW_HEIGHT)];
    for hit in &hits[visible.clone()] {
        rows = rows.push(list_row(app, p, hit));
    }
    rows = rows.push(Space::new().height((hits.len() - visible.end) as f32 * ROW_HEIGHT));

    let header = container(
        row![
            Space::new().width(THUMB_COLUMN - theme::S5),
            heading(p, t("list-name")).width(Length::FillPortion(3)),
            heading(p, t("list-folder")).width(Length::FillPortion(3)),
            heading(p, t("list-parts")).width(PARTS_COLUMN),
            heading(p, t("list-added")).width(ADDED_COLUMN),
        ]
        .spacing(theme::S5)
        .align_y(Alignment::Center),
    )
    .height(HEADER_HEIGHT)
    .padding([0.0, theme::S5])
    .align_y(Alignment::Center);

    let body = scrollable(rows.width(Length::Fill))
        .id(grid_id())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(|v| {
            Message::Scrolled(GridViewport {
                offset: v.absolute_offset().y,
                height: v.bounds().height,
            })
        })
        .style(move |_, status| p.scrollbar(status));

    container(
        container(column![header, super::widgets::hairline(p), body])
            .clip(true)
            .style(move |_| p.pane(false, false)),
    )
    .padding([GRID_PAD_Y, GRID_PAD_X])
    .into()
}

fn heading<'a>(p: Palette, label: String) -> iced::widget::Text<'a> {
    text(label.to_uppercase())
        .size(theme::SIZE_GROUP)
        .font(theme::FONT_STRONG)
        .color(p.n.ink_3)
}

fn list_row<'a>(app: &App, p: Palette, hit: &ModelHit) -> Element<'a, Message> {
    let selected = app.motion.selection(hit.id, app.selected);
    let hovered = app.hovered == Some(hit.id);
    let id = hit.id;
    let thumb = container(
        container(preview(
            app,
            p,
            ThumbTarget::Model(hit.id),
            Look::still(40.0 - 2.0 * theme::S2, 32.0 - 2.0 * theme::S2),
        ))
        .padding(theme::S2)
        .width(40)
        .height(32)
        .style(move |_| p.well()),
    )
    .width(THUMB_COLUMN - theme::S5);
    let content = row![
        thumb,
        text(hit.name.clone())
            .size(theme::SIZE_BODY)
            .font(theme::FONT_STRONG)
            .wrapping(text::Wrapping::None)
            .width(Length::FillPortion(3)),
        text(parent_path(&hit.rel_path).to_owned())
            .size(theme::SIZE_DATA)
            .font(theme::FONT_DATA)
            .color(p.n.ink_3)
            .wrapping(text::Wrapping::None)
            .width(Length::FillPortion(3)),
        text(group(u64::from(hit.part_count)))
            .size(theme::SIZE_BODY)
            .font(theme::FONT_MEDIUM)
            .width(PARTS_COLUMN),
        text(date(hit.created_at))
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(ADDED_COLUMN),
    ]
    .spacing(theme::S5)
    .align_y(Alignment::Center);

    let cell = container(content)
        .height(ROW_HEIGHT - 1.0)
        .padding([0.0, theme::S5])
        .align_y(Alignment::Center)
        .clip(true)
        .style(move |_| p.list_row_fade(selected, hovered));

    mouse_area(
        button(column![cell, super::widgets::hairline(p)])
            .padding(0)
            .style(move |_, _| p.bare())
            .on_press(Message::Select(id)),
    )
    .on_double_click(Message::OpenModel(id))
    .on_enter(Message::Hover(Some(id)))
    .on_exit(Message::Hover(None))
    .into()
}
