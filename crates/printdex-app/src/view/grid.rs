//! The virtualized model grid: only visible rows get widgets.

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, stack, text};
use iced::{Alignment, Element, Length};
use printdex_core::ModelHit;

use crate::app::{App, GRID_PAD_X, GRID_PAD_Y, GridViewport, Message, grid_id};
use crate::format::{group, parent_path};
use crate::grid::{COLUMN_GAP, GridMetrics, PANE_PADDING, ROW_GAP};
use crate::i18n::{Arg, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let m = app.metrics();
    let hits = &app.search.hits;
    let vp = app.grid_viewport();
    let total_rows = m.rows(hits.len());
    let visible = m.visible_rows(vp.offset - GRID_PAD_Y, vp.height, hits.len());
    let stride = m.stride();

    let mut col = column![Space::new().height(GRID_PAD_Y + visible.start as f32 * stride)];
    for r in visible.clone() {
        let start = r * m.columns;
        let end = (start + m.columns).min(hits.len());
        let mut panes = row![].spacing(COLUMN_GAP);
        for hit in &hits[start..end] {
            panes = panes.push(pane(app, p, &m, hit));
        }
        col = col.push(container(panes).height(stride).align_y(Alignment::Start));
    }
    let below = (total_rows - visible.end) as f32 * stride;
    col = col.push(Space::new().height(below - ROW_GAP + GRID_PAD_Y));

    scrollable(
        container(col)
            .padding([0.0, GRID_PAD_X])
            .width(Length::Fill),
    )
    .id(grid_id())
    .width(Length::Fill)
    .height(Length::Fill)
    .on_scroll(|v| {
        Message::Scrolled(GridViewport {
            offset: v.absolute_offset().y,
            height: v.bounds().height,
        })
    })
    .style(move |_, status| p.scrollbar(status))
    .into()
}

fn pane<'a>(app: &App, p: Palette, m: &GridMetrics, hit: &ModelHit) -> Element<'a, Message> {
    let hovered = app.hovered == Some(hit.id);
    let motion = &app.motion;
    let selection = motion.selection(hit.id, app.selected);
    let inner_w = (m.pane_width - 2.0 * PANE_PADDING - 2.0 * theme::S5).max(0.0);
    let inner_h = (m.well_height - 2.0 * theme::S5).max(0.0);

    let mut well = stack![
        container(icons::placeholder_lifted(
            inner_w,
            inner_h,
            motion.lift(hit.id),
            motion.shadow_scale(hit.id),
        ))
        .padding(theme::S5)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| p.well()),
    ]
    .width(Length::Fill)
    .height(m.well_height);
    if hit.part_count > 1 {
        let n = u64::from(hit.part_count);
        well = well.push(
            container(
                container(
                    row![
                        icons::icon(Icon::Layers, theme::ICON_CHIP, p.n.ink_2),
                        text(t_args(
                            "pane-parts",
                            &[("count", Arg::count(n)), ("n", Arg::str(group(n)))],
                        ))
                        .size(theme::SIZE_DATA)
                        .font(theme::FONT_MEDIUM),
                    ]
                    .spacing(theme::S2)
                    .align_y(Alignment::Center),
                )
                .padding([1.0, 7.0])
                .style(move |_| p.chip()),
            )
            .padding(theme::S3)
            .height(Length::Fill)
            .align_y(Alignment::End),
        );
    }

    let folder = parent_path(&hit.rel_path);
    let body = column![
        well,
        container(
            text(hit.name.clone())
                .size(theme::SIZE_BODY)
                .font(theme::FONT_STRONG)
                .line_height(1.3)
        )
        .height(34)
        .clip(true),
        container(
            text(folder.to_owned())
                .size(theme::SIZE_DATA)
                .font(theme::FONT_DATA)
                .color(p.n.ink_3)
                .wrapping(text::Wrapping::None)
        )
        .height(16)
        .clip(true),
    ]
    .spacing(theme::S2);

    let id = hit.id;
    let card = container(body)
        .padding(theme::S4)
        .width(m.pane_width)
        .height(m.row_height)
        .clip(true)
        .style(move |_| p.pane_fade(selection, hovered));
    let card = container(card).padding(iced::Padding {
        top: motion.sink(id),
        ..iced::Padding::ZERO
    });

    mouse_area(
        button(card)
            .padding(0)
            .style(move |_, _| p.bare())
            .on_press(Message::Select(id)),
    )
    .on_double_click(Message::OpenModel(id))
    .on_enter(Message::Hover(Some(id)))
    .on_exit(Message::Hover(None))
    .into()
}
