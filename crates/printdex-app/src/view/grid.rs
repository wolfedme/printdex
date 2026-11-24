//! The virtualized model grid: only visible rows get widgets.

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, stack, text};
use iced::{Alignment, Element, Length};
use printdex_core::{ModelHit, ThumbTarget, format_number};

use crate::app::{App, GRID_PAD_X, GRID_PAD_Y, GridViewport, Message, grid_id};
use crate::format::{group, parent_path};
use crate::grid::{
    CHIP_LINE, COLUMN_GAP, GridMetrics, NAME_LINES, PANE_PADDING, ROW_GAP, STATUS_LINE,
};
use crate::i18n::{Arg, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

use super::preview::{Look, preview};

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
    // One model selected: wash and ring together. Several picked: the
    // wash and a check mark show what's picked, the ring alone shows the
    // keyboard cursor (DESIGN.md "Model pane").
    let picking = !app.files.picked.is_empty();
    let picked = picking && app.picked(hit.id);
    let (wash, ring) = if picking {
        (
            f32::from(u8::from(picked)),
            f32::from(u8::from(app.selected == Some(hit.id))),
        )
    } else {
        let s = motion.selection(hit.id, app.selected);
        (s, s)
    };
    let material = hit.preview_material;
    let inner_w = (m.pane_width - 2.0 * PANE_PADDING - 2.0 * theme::S5).max(0.0);
    let inner_h = (m.well_height - 2.0 * theme::S5).max(0.0);

    let mut well = stack![
        container(preview(
            app,
            p,
            ThumbTarget::Model(hit.id),
            Look {
                lift: motion.lift(hit.id),
                shadow: motion.shadow_scale(hit.id),
                ..Look::still(inner_w, inner_h).labelled()
            },
        ))
        .padding(theme::S5)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| p.material_well(material)),
    ]
    .width(Length::Fill)
    .height(m.well_height);
    if picked {
        well = well.push(
            container(
                container(icons::icon(Icon::Check, theme::ICON_CHIP, p.t.on_primary))
                    .padding(3)
                    .style(move |_| p.check_badge()),
            )
            .padding(theme::S3),
        );
    }
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

    // Name and meta read as one group under the well: the meta follows
    // the name whether it takes one line or two, and the slack a short
    // name leaves goes to the bottom of the pane, not between them.
    let folder = parent_path(&hit.rel_path);
    let name = container(
        row![
            text(hit.name.clone())
                .size(theme::SIZE_BODY)
                .font(theme::FONT_STRONG)
                .line_height(1.3)
                .width(Length::Fill),
            text(hit.number.map(format_number).unwrap_or_default())
                .size(theme::SIZE_DATA)
                .font(theme::FONT_DATA)
                .color(p.n.ink_3)
                .line_height(1.6)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(theme::S3),
    )
    .max_height(NAME_LINES)
    .clip(true);
    let meta = column![
        container(super::prints::card_chips(p, hit).unwrap_or_else(|| {
            // A folder is a word, not measured data: Figtree, not mono.
            // Models at the library root have no folder to show.
            if folder.is_empty() {
                return Space::new().into();
            }
            row![
                icons::icon(Icon::Folder, theme::ICON_CHIP, p.n.ink_3),
                text(folder.to_owned())
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_3)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(theme::S2)
            .align_y(Alignment::Center)
            .into()
        }))
        .height(CHIP_LINE)
        .align_y(Alignment::Center)
        .clip(true),
        container(super::prints::status_line(p, hit.last_print.as_ref()))
            .height(STATUS_LINE)
            .align_y(Alignment::Center)
            .clip(true),
    ]
    .spacing(theme::S2);
    let body = column![well, column![name, meta].spacing(theme::S2)].spacing(theme::S4);

    let id = hit.id;
    let card = container(body)
        .padding(theme::S4)
        .width(m.pane_width)
        .height(m.row_height)
        .clip(true)
        .style(move |_| p.pane_look(wash, ring, hovered));
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
