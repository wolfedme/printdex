//! Model view: one model's parts, other files and details.

use iced::widget::{Space, button, column, container, image, row, scrollable, stack, text};
use iced::{Alignment, ContentFit, Element, Length};
use printdex_core::{
    Detail, ModelDetail, ModelKind, PartInfo, PrintableFormat, ThumbFailure, ThumbTarget,
    format_number,
};

use super::organize;
use super::preview::{self, Look};
use super::statusbar::hint;
use super::viewer::{self, Scene};
use super::widgets::{self, date, icon_button};
use crate::app::{App, Message, ModelView};
use crate::format::{file_name, group, parent_path, size};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};
use crate::viewer::Load;

/// Width of the details column.
const SIDE_WIDTH: f32 = 384.0;
/// Height of the large preview well.
const PREVIEW_HEIGHT: f32 = 340.0;
/// Width of the preview inside the well (4:3 of its inner height).
const PREVIEW_WIDTH: f32 = (PREVIEW_HEIGHT - 2.0 * theme::S9) * 4.0 / 3.0;
const TILE_WIDTH: f32 = 112.0;
const TILE_WELL: f32 = 76.0;

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    match &app.model {
        Some(ModelView::Loaded { detail, part }) => loaded(app, p, detail, *part),
        Some(ModelView::Missing(_)) => missing(p),
        Some(ModelView::Loading(_)) | None => container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| p.ground())
            .into(),
    }
}

/// A length in mm for the dimension readout: one decimal below 100 mm.
fn mm(v: f32) -> String {
    if v < 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// The large preview: the interactive 3D viewer once the part's mesh is
/// loaded, the thumbnail until then. The flag says whether the viewer
/// shows (it fills the well with less padding).
fn preview_well<'a>(app: &'a App, p: Palette, pt: &PartInfo) -> (Element<'a, Message>, bool) {
    let thumb = |label: Option<String>| -> Element<'a, Message> {
        let picture = preview::preview(
            app,
            p,
            ThumbTarget::Part(pt.id),
            Look::still(PREVIEW_WIDTH, PREVIEW_HEIGHT - 2.0 * theme::S9).labelled(),
        );
        match label {
            Some(l) => stack![
                picture,
                container(note(p, l))
                    .align_bottom(Length::Fill)
                    .align_right(Length::Fill),
            ]
            .into(),
            None => picture,
        }
    };
    let v = &app.viewer;
    let loaded = match &v.load {
        Load::Ready(l) => l,
        Load::Loading(_) => return (thumb(Some(t("viewer-loading"))), false),
        Load::Failed(ThumbFailure::TooLarge) => {
            return (thumb(Some(t("preview-too-large"))), false);
        }
        Load::Failed(_) => return (thumb(Some(t("viewer-unavailable"))), false),
        Load::Idle => return (thumb(None), false),
    };
    let base: Element<'a, Message> = if v.software {
        match &v.picture {
            Some(h) => image(h.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Contain)
                .into(),
            None => container(thumb(None)).center(Length::Fill).into(),
        }
    } else {
        Space::new().into()
    };
    let shader =
        viewer::shader_view(Some(Scene::new(loaded, v.view, v.material))).map(Message::Viewer);
    let mut actions = row![Space::new().width(Length::Fill)].spacing(theme::S3);
    if loaded.decimated && loaded.detail == Detail::Preview {
        actions = actions.push(small_button(
            p,
            t("viewer-full-detail"),
            Message::Viewer(crate::viewer::Msg::FullDetail),
        ));
    }
    if v.touched {
        actions = actions.push(small_button(
            p,
            t("viewer-reset"),
            Message::Viewer(crate::viewer::Msg::Reset),
        ));
    }
    let overlay = container(actions).padding(theme::S4);
    let layers = stack![base, shader, overlay]
        .width(Length::Fill)
        .height(Length::Fill);
    (layers.into(), true)
}

fn note<'a>(p: Palette, s: String) -> Element<'a, Message> {
    text(s)
        .size(theme::SIZE_DATA)
        .font(theme::FONT_DATA)
        .color(p.n.ink_3)
        .into()
}

fn small_button<'a>(p: Palette, label: String, msg: Message) -> Element<'a, Message> {
    button(text(label).size(theme::SIZE_CAPTION))
        .padding([theme::S2, theme::S4])
        .style(move |_, status| p.button_secondary(status))
        .on_press(msg)
        .into()
}

fn format_label(f: PrintableFormat) -> &'static str {
    match f {
        PrintableFormat::Stl => "STL",
        PrintableFormat::ThreeMf => "3MF",
        PrintableFormat::Obj => "OBJ",
    }
}

fn loaded<'a>(app: &'a App, p: Palette, d: &'a ModelDetail, part: usize) -> Element<'a, Message> {
    let lib_name = app
        .library(d.library_id)
        .map(|l| l.display_name.clone())
        .unwrap_or_default();

    // Header: breadcrumb, name, actions.
    let mut crumbs = row![
        icons::icon(Icon::Folder, theme::ICON_DENSE, p.n.ink_3),
        text(lib_name.clone()).size(theme::SIZE_CAPTION),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);
    for seg in parent_path(&d.rel_path)
        .split('/')
        .filter(|s| !s.is_empty())
    {
        crumbs = crumbs
            .push(icons::icon(Icon::Chevron, theme::ICON_CHIP, p.n.ink_3))
            .push(text(seg.to_owned()).size(theme::SIZE_CAPTION));
    }
    let current = d.parts.get(part);
    let reveal = app.model_path(&d.rel_path).map(Message::ShowInFolder);
    let open = current
        .and_then(|pt| app.model_path(&pt.rel_path))
        .map(Message::OpenFile);
    let header = container(
        column![
            crumbs,
            row![
                text(d.name.clone())
                    .size(theme::SIZE_DISPLAY)
                    .font(theme::FONT_BOLD)
                    .color(p.n.ink)
                    .wrapping(text::Wrapping::None),
                text(d.number.map(format_number).unwrap_or_default())
                    .size(theme::SIZE_BODY)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_3),
                Space::new().width(Length::Fill),
                icon_button(
                    p,
                    Icon::FolderOpen,
                    t("model-show-in-folder"),
                    Palette::button_ghost,
                    reveal
                ),
                icon_button(
                    p,
                    Icon::ExternalLink,
                    t("model-open"),
                    Palette::button_secondary,
                    open
                ),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::S2),
    )
    .padding([theme::S5, theme::S8])
    .width(Length::Fill)
    .style(move |_| p.bar());

    // Main column: preview, parts, other files.
    let caption = current.map_or_else(String::new, |pt| {
        let mut parts = vec![
            file_name(&pt.rel_path).to_owned(),
            size(pt.size),
            format_label(pt.format).to_owned(),
        ];
        // The 3D preview measures as it loads; the database once the
        // thumbnail has been rendered.
        let loaded = match &app.viewer.load {
            Load::Ready(l) if app.viewer.part == Some(pt.id) => Some(l),
            _ => None,
        };
        let dims = loaded.map(|l| l.dims).or(pt.dims);
        let triangles = loaded.map(|l| l.triangles).or(pt.triangles);
        if let Some([x, y, z]) = dims {
            parts.push(t_args(
                "model-measured",
                &[
                    ("x", Arg::str(mm(x))),
                    ("y", Arg::str(mm(y))),
                    ("z", Arg::str(mm(z))),
                ],
            ));
        }
        if let Some(n) = triangles {
            parts.push(t_args(
                "model-triangles",
                &[("count", Arg::count(n)), ("n", Arg::str(group(n)))],
            ));
        }
        if let Some(l) = loaded.filter(|l| l.decimated) {
            parts.push(t_args(
                "viewer-simplified",
                &[("shown", Arg::str(group(l.mesh.triangles() as u64)))],
            ));
        }
        parts.join(" · ")
    });
    let (well, three_d) = match current {
        Some(pt) => preview_well(app, p, pt),
        None => (icons::placeholder().into(), false),
    };
    let preview = container(
        column![
            container(well)
                .padding(if three_d { theme::S2 } else { theme::S9 })
                .center_x(Length::Fill)
                .height(PREVIEW_HEIGHT)
                .style(move |_| p.well()),
            text(caption)
                .size(theme::SIZE_DATA)
                .font(theme::FONT_DATA)
                .color(p.n.ink_3),
        ]
        .spacing(theme::S4),
    )
    .padding(theme::S5)
    .style(move |_| p.pane(false, false));

    let parts_size: u64 = d.parts.iter().map(|pt| pt.size).sum();
    let n = d.parts.len() as u64;
    let mut tiles = row![].spacing(theme::S4);
    for (i, pt) in d.parts.iter().enumerate() {
        tiles = tiles.push(tile(app, p, pt, app.motion.part(i, part), i));
    }
    let mut main = column![
        preview,
        widgets::section(
            p,
            t("model-parts"),
            Some(t_args(
                "model-parts-summary",
                &[
                    ("count", Arg::count(n)),
                    ("n", Arg::str(group(n))),
                    ("size", Arg::str(size(parts_size))),
                ],
            )),
        ),
        tiles.wrap().vertical_spacing(theme::S4),
        organize::notes(&app.org, p, d.id),
    ]
    .spacing(theme::S5);

    if !d.assets.is_empty() {
        let mut files = column![].spacing(0);
        for (i, a) in d.assets.iter().enumerate() {
            if i > 0 {
                files = files.push(widgets::hairline(p));
            }
            files = files.push(asset_row(app, p, &a.rel_path, a.size));
        }
        main = main
            .push(widgets::section(p, t("model-other-files"), None))
            .push(
                container(files)
                    .padding([theme::S2, theme::S5])
                    .style(move |_| p.pane(false, false)),
            );
    }

    // Side column: details.
    let kind = match d.kind {
        ModelKind::Folder => t("model-kind-folder"),
        ModelKind::SingleFile => t("model-kind-file"),
    };
    let folder = if d.kind == ModelKind::Folder {
        d.rel_path.clone()
    } else {
        parent_path(&d.rel_path).to_owned()
    };
    let organize = container(organize::side(&app.org, p, d.id))
        .padding(theme::S6)
        .width(SIDE_WIDTH)
        .style(move |_| p.pane(false, false));
    let details = container(
        column![
            widgets::section(p, t("model-details"), None),
            detail_row(p, t("model-detail-library"), lib_name, false),
            detail_row(p, t("model-detail-folder"), format!("/{folder}"), true),
            detail_row(p, t("model-detail-parts"), group(n), false),
            detail_row(p, t("model-detail-size"), size(d.total_size()), false),
            detail_row(p, t("model-detail-added"), date(d.created_at), false),
            detail_row(p, t("model-detail-kind"), kind, false),
        ]
        .spacing(theme::S4),
    )
    .padding(theme::S6)
    .width(SIDE_WIDTH)
    .style(move |_| p.pane(false, false));

    let body = scrollable(
        row![
            main.width(Length::Fill),
            column![organize, details].spacing(theme::S6)
        ]
        .spacing(theme::S8)
        .padding([theme::S8, theme::S8]),
    )
    .height(Length::Fill)
    .style(move |_, status| p.scrollbar(status));

    let status = container(
        row![
            hint(p, "Esc", t("status-key-back")),
            hint(p, "←→", t("status-key-part")),
            hint(p, "O", t("status-key-open-file")),
            hint(p, "T", t("status-key-tag")),
            hint(p, "N", t("status-key-notes")),
            text(t("status-key-turn"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
            Space::new().width(Length::Fill),
            text(t_args(
                "status-added",
                &[("date", Arg::str(date(d.created_at)))]
            ))
            .size(theme::SIZE_CAPTION)
            .font(theme::FONT_MEDIUM),
        ]
        .spacing(theme::S6)
        .align_y(Alignment::Center),
    )
    .height(theme::STATUSBAR_HEIGHT)
    .padding([0.0, theme::S7])
    .align_y(Alignment::Center)
    .style(move |_| p.bar());

    column![
        header,
        widgets::hairline(p),
        container(body)
            .height(Length::Fill)
            .style(move |_| p.ground()),
        widgets::hairline(p),
        status,
    ]
    .into()
}

fn tile<'a>(
    app: &App,
    p: Palette,
    pt: &PartInfo,
    selected: f32,
    index: usize,
) -> Element<'a, Message> {
    let inner = TILE_WIDTH - 2.0 * theme::S3 - 2.0 * theme::S4;
    let body = column![
        container(preview::preview(
            app,
            p,
            ThumbTarget::Part(pt.id),
            Look::still(inner, TILE_WELL - 2.0 * theme::S4),
        ))
        .padding(theme::S4)
        .width(Length::Fill)
        .height(TILE_WELL)
        .style(move |_| p.well()),
        container(
            text(file_name(&pt.rel_path).to_owned())
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_MEDIUM)
                .wrapping(text::Wrapping::None)
        )
        .clip(true),
        text(size(pt.size))
            .size(theme::SIZE_DATA)
            .font(theme::FONT_DATA)
            .color(p.n.ink_3),
    ]
    .spacing(theme::S2);
    button(
        container(body)
            .padding(theme::S3)
            .width(TILE_WIDTH)
            .clip(true)
            .style(move |_| p.pane_fade(selected, false)),
    )
    .padding(0)
    .style(move |_, _| p.bare())
    .on_press(Message::SelectPart(index))
    .into()
}

fn asset_row<'a>(app: &App, p: Palette, rel: &str, bytes: u64) -> Element<'a, Message> {
    let name = file_name(rel);
    let lower = name.to_ascii_lowercase();
    let icon = if [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        Icon::Image
    } else if [".pdf", ".txt", ".md"].iter().any(|e| lower.ends_with(e))
        || lower.starts_with("readme")
    {
        Icon::FileText
    } else {
        Icon::File
    };
    let open = app.model_path(rel).map(Message::OpenFile);
    row![
        icons::icon(icon, theme::ICON, p.n.ink_3),
        text(name.to_owned())
            .size(theme::SIZE_BODY)
            .wrapping(text::Wrapping::None)
            .width(Length::Fill),
        text(size(bytes))
            .size(theme::SIZE_DATA)
            .font(theme::FONT_DATA)
            .color(p.n.ink_3),
        button(text(t("model-open")).size(theme::SIZE_CAPTION))
            .padding([2.0, theme::S4])
            .style(move |_, status| p.button_ghost(status))
            .on_press_maybe(open),
    ]
    .spacing(theme::S4)
    .padding([theme::S3, 0.0])
    .align_y(Alignment::Center)
    .into()
}

fn detail_row<'a>(p: Palette, label: String, value: String, mono: bool) -> Element<'a, Message> {
    let value = text(value)
        .size(if mono {
            theme::SIZE_DATA
        } else {
            theme::SIZE_BODY
        })
        .font(if mono {
            theme::FONT_DATA
        } else {
            theme::FONT_MEDIUM
        })
        .color(p.n.ink);
    row![
        text(label)
            .size(theme::SIZE_CAPTION)
            .color(p.n.ink_3)
            .width(110),
        value.width(Length::Fill),
    ]
    .spacing(theme::S4)
    .align_y(Alignment::Center)
    .into()
}

fn missing<'a>(p: Palette) -> Element<'a, Message> {
    container(
        column![
            icons::icon(Icon::Alert, theme::S9, p.n.ink_3),
            text(t("model-missing-title"))
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_BOLD)
                .color(p.n.ink),
            text(t("model-missing-body"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2),
            button(text(t("model-back")).size(theme::SIZE_BODY))
                .height(theme::CONTROL_HEIGHT)
                .padding([8.0, theme::S5])
                .style(move |_, status| p.button_secondary(status))
                .on_press(Message::Back),
        ]
        .spacing(theme::S5)
        .align_x(Alignment::Center),
    )
    .center(Length::Fill)
    .style(move |_| p.ground())
    .into()
}
