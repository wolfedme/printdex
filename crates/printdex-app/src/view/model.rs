//! Model view: one model's parts, other files and details.

use iced::widget::{Space, button, column, container, image, row, scrollable, stack, text};
use iced::{Alignment, ContentFit, Element, Length};
use printdex_core::{
    Detail, ModelDetail, ModelKind, PartInfo, PrintableFormat, ThumbFailure, ThumbTarget,
    format_number,
};

use super::organize;
use super::preview::{self, Look};
use super::prints;
use super::statusbar::hint;
use super::viewer::{self, Scene};
use super::widgets::{self, date, icon_button};
use crate::app::{App, Message, ModelView};
use crate::files::Fx;
use crate::format::{file_name, group, parent_path, size};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::org::Org;
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
    // A missing model's files are gone: its last thumbnail, if cached, and
    // no 3D view.
    if matches!(&app.model, Some(ModelView::Loaded { detail, .. }) if detail.missing_since.is_some())
    {
        let picture = preview::preview(
            app,
            p,
            ThumbTarget::Part(pt.id),
            Look::still(PREVIEW_WIDTH, PREVIEW_HEIGHT - 2.0 * theme::S9),
        );
        return (picture, false);
    }
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

/// The primary "Log a print" button in the header.
fn log_button<'a>(p: Palette, enabled: bool) -> Element<'a, Message> {
    button(
        row![
            icons::icon(Icon::LogPrint, theme::ICON, p.t.on_primary),
            text(t("log-a-print")).size(theme::SIZE_BODY),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .height(theme::CONTROL_HEIGHT)
    .padding([8.0, theme::S5])
    .style(move |_, status| p.button_primary(status))
    .on_press_maybe(enabled.then_some(Message::Prints(crate::prints::Pr::OpenLog)))
    .into()
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
    let gone = d.missing_since.is_some();
    let compact = app.compact();
    let reveal = app
        .model_path(&d.rel_path)
        .filter(|_| !gone)
        .map(Message::ShowInFolder);
    let open = current
        .filter(|_| !gone && app.model_path(&d.rel_path).is_some())
        .map(|_| Message::OpenPart);
    let header = container(
        column![
            crumbs,
            row![
                // A long name is clipped rather than pushing the actions
                // (and Log a print) out of the window.
                container(
                    row![
                        text(d.name.clone())
                            .size(theme::SIZE_DISPLAY)
                            .font(theme::FONT_BOLD)
                            .color(p.n.ink)
                            .wrapping(text::Wrapping::None),
                        text(d.number.map(format_number).unwrap_or_default())
                            .size(theme::SIZE_BODY)
                            .font(theme::FONT_DATA)
                            .color(p.n.ink_3)
                            .wrapping(text::Wrapping::None),
                    ]
                    .spacing(theme::S4)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .clip(true),
                if compact {
                    tip(p, Icon::FolderOpen, t("model-show-in-folder"), reveal)
                } else {
                    icon_button(
                        p,
                        Icon::FolderOpen,
                        t("model-show-in-folder"),
                        Palette::button_ghost,
                        reveal,
                    )
                },
                file_actions(p, gone),
                restore_button(app, p, d.id),
                open_button(app, p, open),
                log_button(p, app.prints.form.is_none()),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::S2),
    )
    .padding([theme::S5, theme::S8])
    .width(Length::Fill)
    .style(move |_| p.bar());
    let mut header = column![header];
    if let Some(since) = d.missing_since {
        header = header.push(missing_banner(app, p, d, since));
    }
    if let Some(panel) = super::restore::model_panel(app, p, d) {
        header = header.push(panel);
    }
    let header: Element<'a, Message> = header.into();

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
    let data = app
        .org
        .data
        .as_ref()
        .filter(|(id, _)| *id == d.id)
        .map(|(_, data)| data);
    let mut side = column![].spacing(theme::S6).width(SIDE_WIDTH);
    side = match (&app.prints.form, data) {
        (Some(f), _) => side.push(prints::form(app, p, f)),
        (None, Some(data)) => side
            // The core loop first: how to print it, how it went.
            .push(prints::recipe_card(p, data))
            .push(prints::print_log(app, p, data))
            .push(organize)
            .push(details),
        (None, None) => side.push(organize).push(details),
    };

    let body = scrollable(
        row![main.width(Length::Fill), side]
            .spacing(theme::S8)
            .padding([theme::S8, theme::S8]),
    )
    .height(Length::Fill)
    .style(move |_, status| p.scrollbar(status));

    // Key hints only; the added date lives in Details. The viewer hint
    // goes first when the window narrows.
    let mut hints = row![
        hint(p, "Esc", t("status-key-back")),
        hint(p, "←→", t("status-key-part")),
        hint(p, "O", t("status-key-open-file")),
        hint(p, "T", t("status-key-tag")),
        hint(p, "N", t("status-key-notes")),
        hint(p, "L", t("status-key-log")),
    ]
    .spacing(theme::S6)
    .align_y(Alignment::Center);
    if !compact {
        hints = hints.push(
            text(t("status-key-turn"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    let status = container(hints)
        .width(Length::Fill)
        .height(theme::STATUSBAR_HEIGHT)
        .padding([0.0, theme::S7])
        .align_y(Alignment::Center)
        .style(move |_| p.bar());

    column![
        header,
        widgets::hairline(p),
        stack![
            container(body)
                .height(Length::Fill)
                .style(move |_| p.ground()),
            // "Attempt 4 saved" floats over the bottom of the side column.
            match app.prints.toast.as_deref() {
                Some(m) => container(container(prints::toast(p, m)).width(SIDE_WIDTH))
                    .padding(theme::S8)
                    .align_bottom(Length::Fill)
                    .align_right(Length::Fill)
                    .into(),
                None => Element::from(Space::new()),
            },
        ]
        .height(Length::Fill),
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

/// Rename, Move to…, Move to trash: icon buttons with tooltips.
fn file_actions<'a>(p: Palette, gone: bool) -> Element<'a, Message> {
    let msg = |f: Fx| (!gone).then_some(Message::Files(f));
    row![
        tip(p, Icon::Rename, t("files-rename"), msg(Fx::AskRename)),
        tip(p, Icon::Move, t("files-move"), msg(Fx::AskMove)),
        tip(p, Icon::Remove, t("files-trash"), msg(Fx::AskTrash)),
    ]
    .spacing(theme::S1)
    .into()
}

fn tip<'a>(p: Palette, icon: Icon, label: String, msg: Option<Message>) -> Element<'a, Message> {
    let fg = if msg.is_some() { p.n.ink_2 } else { p.n.ink_3 };
    iced::widget::tooltip(
        button(icons::icon(icon, theme::ICON, fg))
            .height(theme::CONTROL_HEIGHT)
            .padding([8.0, theme::S3])
            .style(move |_, status| p.button_ghost(status))
            .on_press_maybe(msg),
        container(text(label).size(theme::SIZE_CAPTION))
            .padding([3.0, theme::S3])
            .style(move |_| p.tooltip()),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

/// Open (in the chosen app: "Open in PrusaSlicer"), with a menu for the
/// default app and choosing one.
fn open_button<'a>(app: &'a App, p: Palette, open: Option<Message>) -> Element<'a, Message> {
    let label = match app.open_with() {
        Some(a) => t_args(
            "model-open-in",
            &[("app", Arg::str(crate::apps::app_name(a)))],
        ),
        None => t("model-open"),
    };
    let enabled = open.is_some();
    let main = icon_button(
        p,
        Icon::ExternalLink,
        label,
        Palette::button_secondary,
        open,
    );
    let chevron = button(icons::icon(
        Icon::ChevronDown,
        theme::ICON_DENSE,
        if enabled { p.n.ink_2 } else { p.n.ink_3 },
    ))
    .height(theme::CONTROL_HEIGHT)
    .padding([8.0, theme::S2])
    .style(move |_, status| p.button_secondary(status))
    .on_press_maybe(enabled.then_some(Message::Files(Fx::ToggleOpenMenu)));
    let r = row![main, chevron].spacing(1);
    let menu = app.files.open_menu.then(|| {
        let format = app.current_part().map(|pt| pt.format);
        let mut menu = column![].spacing(theme::S1);
        if app.open_with().is_some() {
            menu = menu.push(menu_item(
                p,
                Icon::ExternalLink,
                t("model-open-default"),
                Message::Files(Fx::OpenDefault),
            ));
        }
        if let Some(f) = format {
            menu = menu.push(menu_item(
                p,
                Icon::App,
                t_args(
                    "model-open-choose",
                    &[("format", Arg::str(format_label(f).to_owned()))],
                ),
                Message::Files(Fx::ChooseApp(f)),
            ));
        }
        container(menu)
            .padding(theme::S2)
            .width(Length::Fill)
            .style(move |_| p.dialog())
            .into()
    });
    super::dropdown::dropdown(r, menu, OPEN_MENU_WIDTH)
        .on_dismiss(Message::Files(Fx::CloseOpenMenu))
        .into()
}

/// Width of the Open menu.
const OPEN_MENU_WIDTH: f32 = 280.0;

fn menu_item<'a>(p: Palette, icon: Icon, label: String, msg: Message) -> Element<'a, Message> {
    button(
        row![
            icons::icon(icon, theme::ICON_DENSE, p.n.ink_2),
            text(label).size(theme::SIZE_CAPTION),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([5.0, theme::S4])
    .style(move |_, status| p.button_ghost(status))
    .on_press(msg)
    .into()
}

/// Under the header of a missing model: since when, where it was, and
/// Forget (asks first).
fn missing_banner<'a>(
    app: &'a App,
    p: Palette,
    d: &'a ModelDetail,
    since: i64,
) -> Element<'a, Message> {
    let where_it_was = app.model_path(&d.rel_path).map(Message::ShowWhereItWas);
    let content: Element<'a, Message> = if app.org.confirm_forget {
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(t_args(
                "model-forget-confirm",
                &[("name", Arg::str(d.name.clone()))]
            ))
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
            small_button(p, t("settings-cancel"), Message::Org(Org::Cancel)),
            button(
                text(t("model-forget-yes"))
                    .size(theme::SIZE_CAPTION)
                    .font(theme::FONT_STRONG)
            )
            .padding([5.0, theme::S4])
            .style(move |_, status| p.button_danger(status))
            .on_press(Message::Org(Org::ConfirmForget(d.id))),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center)
        .into()
    } else {
        let mut r = row![
            icons::icon(Icon::Offline, theme::ICON, p.status.offline),
            text(t_args(
                "model-missing-since",
                &[("date", Arg::str(date(since)))]
            ))
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center);
        if let Some(msg) = where_it_was {
            r = r.push(small_button(p, t("model-where-it-was"), msg));
        }
        if app.backup.target().is_some() {
            r = r.push(small_button(
                p,
                t("restore-model"),
                Message::Restore(crate::restore_ui::Rx::OpenModel(d.id)),
            ));
        }
        r.push(small_button(
            p,
            t("model-forget"),
            Message::Org(Org::AskForget),
        ))
        .into()
    };
    container(content)
        .padding([theme::S4, theme::S8])
        .width(Length::Fill)
        .style(move |_| p.error_line())
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

/// "Restore from backup…" in the header, once backups are set up.
fn restore_button<'a>(app: &App, p: Palette, id: printdex_core::ModelId) -> Element<'a, Message> {
    if app.backup.target().is_none() {
        return Space::new().into();
    }
    icon_button(
        p,
        Icon::Backup,
        t("restore-model"),
        Palette::button_ghost,
        Some(Message::Restore(crate::restore_ui::Rx::OpenModel(id))),
    )
}
