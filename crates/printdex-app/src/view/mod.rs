//! Views: turn [`App`] state into widgets. No logic beyond layout.

mod backup;
mod dropdown;
mod empty;
mod files;
mod focus;
mod grid;
mod list;
mod model;
mod organize;
mod preview;
mod prints;
mod restore;
pub mod settings;
mod sidebar;
mod statusbar;
mod sync;
mod toolbar;
mod viewer;
mod widgets;

pub use organize::notes_editor_id;

use focus::button;
use iced::widget::{Space, column, container, row, stack, text};
use iced::{Alignment, Element, Length};

use crate::app::{
    App, Boot, BootFailure, BootUx, CrashUx, Message, Screen, boot_failure_text, task_stopped_text,
};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::settings::ViewMode;
use crate::theme::{self, Palette};

pub fn view(app: &App) -> Element<'_, Message> {
    let p = app.palette();
    focus::set_accent(p.t.accent);
    match &app.boot {
        Boot::Opening => centered(
            p,
            text(t("app-opening"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2),
        ),
        Boot::Failed { dir, reason } => centered(p, boot_failed(p, dir, *reason)),
        Boot::Ready => shell(app, p),
    }
}

fn shell(app: &App, p: Palette) -> Element<'_, Message> {
    let main = match app.screen {
        Screen::Model(_) => model::view(app, p),
        Screen::Settings => settings::view(app, p),
        Screen::Library => library(app, p),
    };
    let mut r = row![
        sidebar::view(app, p),
        container(Space::new().width(1).height(Length::Fill)).style(move |_| p.hairline()),
        main,
    ];
    if !app.viewer.software {
        // Finds out on the first frame whether shaders can draw.
        r = r.push(viewer::software_probe(Message::Viewer(
            crate::viewer::Msg::Software,
        )));
    }
    let mut layers = stack![r];
    if let Some(toast) = files::toast(app, p) {
        layers = layers.push(toast);
    }
    if let Some(toasts) = task_stopped(app, p) {
        layers = layers.push(toasts);
    }
    if let Some(dialog) = files::dialog(app, p) {
        layers = layers.push(dialog);
    }
    layers.into()
}

fn library(app: &App, p: Palette) -> Element<'_, Message> {
    let mut content = column![].height(Length::Fill);
    if let Some(error) = &app.error {
        content = content.push(container(error_line(p, error)).padding([theme::S5, theme::S7]));
    }
    let no_library = app.libraries.is_empty() && app.search.loaded;
    if !no_library {
        content = content.push(toolbar::header(app, p));
    }
    let filtered = !app.search.query.text.trim().is_empty() || !app.chips.is_empty();
    content = content.push(if no_library {
        empty::no_library(app, p)
    } else if app.search.loaded && app.search.hits.is_empty() && filtered {
        empty::no_results(p, &app.search.query.text, !app.chips.is_empty())
    } else if app.search.loaded && app.search.hits.is_empty() && app.scans.is_empty() {
        empty::empty_scope(p, &app.search.query.scope)
    } else if app.settings.view == ViewMode::List {
        list::view(app, p)
    } else {
        grid::view(app, p)
    });

    let mut top = column![];
    if let Some(b) = update_banner(app, p) {
        top = top.push(b).push(hairline(p));
    }
    if let Some(b) = crash_notice(app, p) {
        top = top.push(b).push(hairline(p));
    }
    if let Some(b) = restore::relink_banner(app, p) {
        top = top.push(b).push(hairline(p));
    }
    let mut top = top.push(toolbar::view(app, p)).push(hairline(p));
    if app.filter_menu {
        top = top.push(toolbar::filter_bar(app, p)).push(hairline(p));
    }
    let mut col = column![
        top,
        container(content)
            .height(Length::Fill)
            .style(move |_| p.ground()),
        hairline(p),
    ];
    if let Some(bar) = files::selection_bar(app, p) {
        col = col.push(bar).push(hairline(p));
    }
    col.push(statusbar::view(app, p)).into()
}

/// The "could not open" screen: what happened and what to do about it.
fn boot_failed<'a>(p: Palette, dir: &std::path::Path, reason: BootFailure) -> Element<'a, Message> {
    let (title, body) = boot_failure_text(reason, dir);
    let button_for = |label: &str, ux: BootUx, primary: bool| -> Element<'a, Message> {
        button(text(t(label)).size(theme::SIZE_BODY))
            .padding([5.0, theme::S4])
            .style(move |_, status| {
                if primary {
                    p.button_primary(status)
                } else {
                    p.button_ghost(status)
                }
            })
            .on_press(Message::BootAction(ux))
            .into()
    };
    let actions: Vec<Element<'a, Message>> = match reason {
        BootFailure::TooNew | BootFailure::ReadOnly => {
            vec![button_for("boot-show-folder", BootUx::ShowFolder, true)]
        }
        BootFailure::Corrupt => vec![
            button_for("boot-set-aside", BootUx::SetAside, true),
            button_for("boot-show-folder", BootUx::ShowFolder, false),
            button_for("crash-report", BootUx::Report, false),
        ],
        BootFailure::DiskFull => vec![
            button_for("boot-try-again", BootUx::Retry, true),
            button_for("boot-show-folder", BootUx::ShowFolder, false),
        ],
        BootFailure::Other => vec![button_for("crash-report", BootUx::Report, true)],
    };
    let mut buttons = row![].spacing(theme::S3).align_y(Alignment::Center);
    for a in actions {
        buttons = buttons.push(a);
    }
    column![
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(title)
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_STRONG)
                .color(p.n.ink),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
        text(body).size(theme::SIZE_BODY).color(p.n.ink_2),
        buttons,
    ]
    .spacing(theme::S5)
    .max_width(BOOT_FAILED_WIDTH)
    .into()
}

const BOOT_FAILED_WIDTH: f32 = 520.0;

fn centered<'a>(p: Palette, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .center(Length::Fill)
        .style(move |_| p.ground())
        .into()
}

fn hairline<'a>(p: Palette) -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
        .style(move |_| p.hairline())
        .into()
}

fn error_line<'a>(p: Palette, message: &str) -> Element<'a, Message> {
    container(
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(message.to_owned())
                .size(theme::SIZE_CAPTION)
                .width(Length::Fill),
            button(text("×").size(theme::SIZE_SECTION))
                .padding([0, 6])
                .style(move |_, _| p.bare())
                .on_press(Message::DismissError),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .padding([theme::S3, theme::S5])
    .style(move |_| p.error_line())
    .into()
}

/// One line over the library: the first-launch "Check for updates
/// automatically?" question, or "Restart to update" once one is ready.
fn update_banner(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    use crate::updates::{UpdateState, Ux};
    let (message, actions): (String, Vec<Element<'_, Message>>) =
        if let UpdateState::Ready(info) = &app.updates.state {
            (
                t_args(
                    "updates-ready",
                    &[("version", Arg::str(info.TargetFullRelease.Version.clone()))],
                ),
                vec![widgets::icon_button(
                    p,
                    Icon::Refresh,
                    t("updates-restart"),
                    Palette::button_primary,
                    Some(Message::Updates(Ux::Restart)),
                )],
            )
        } else if app.ask_update_consent() {
            (
                t("updates-ask"),
                vec![
                    widgets::icon_button(
                        p,
                        Icon::Refresh,
                        t("updates-yes"),
                        Palette::button_primary,
                        Some(Message::Updates(Ux::Consent(true))),
                    ),
                    button(text(t("updates-not-now")).size(theme::SIZE_CAPTION))
                        .padding([5.0, theme::S4])
                        .style(move |_, status| p.button_ghost(status))
                        .on_press(Message::Updates(Ux::Consent(false)))
                        .into(),
                ],
            )
        } else {
            return None;
        };
    let mut r = row![
        text(message)
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);
    for a in actions {
        r = r.push(a);
    }
    Some(
        container(r)
            .padding([theme::S3, theme::S7])
            .width(Length::Fill)
            .style(move |_| p.bar())
            .into(),
    )
}

/// Bottom right, one per engine task that stopped after repeated errors:
/// stays until dismissed, with the crash notice's "Report a bug".
pub fn task_stopped(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    if app.stopped.is_empty() {
        return None;
    }
    let mut cards = column![].spacing(theme::S3).align_x(Alignment::End);
    for &task in &app.stopped {
        let r = row![
            icons::icon(Icon::Alert, theme::ICON_DENSE, p.status.fail),
            text(task_stopped_text(task))
                .size(theme::SIZE_CAPTION)
                .width(Length::Fill),
            button(text(t("crash-report")).size(theme::SIZE_CAPTION))
                .padding([3.0, theme::S3])
                .style(move |_, status| p.button_primary(status))
                .on_press(Message::CrashNotice(CrashUx::Report)),
            button(text(t("crash-dismiss")).size(theme::SIZE_CAPTION))
                .padding([3.0, theme::S3])
                .style(move |_, status| p.button_ghost(status))
                .on_press(Message::DismissTaskStopped(task)),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center);
        cards = cards.push(
            container(r)
                .padding([theme::S4, theme::S5])
                .width(TASK_TOAST_WIDTH)
                .style(move |_| p.dialog()),
        );
    }
    Some(
        container(cards)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::End)
            .padding(theme::S8)
            .into(),
    )
}

const TASK_TOAST_WIDTH: f32 = 460.0;

/// Over the library after a crash: says so, with where to read about it.
pub fn crash_notice(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    app.crash.as_ref()?;
    let ghost = |label: String, ux: CrashUx| -> Element<'_, Message> {
        button(text(label).size(theme::SIZE_CAPTION))
            .padding([5.0, theme::S4])
            .style(move |_, status| p.button_ghost(status))
            .on_press(Message::CrashNotice(ux))
            .into()
    };
    let r = row![
        icons::icon(Icon::Alert, theme::ICON, p.status.partial),
        text(t("crash-notice"))
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
        ghost(t("crash-show"), CrashUx::Show),
        button(text(t("crash-report")).size(theme::SIZE_CAPTION))
            .padding([5.0, theme::S4])
            .style(move |_, status| p.button_primary(status))
            .on_press(Message::CrashNotice(CrashUx::Report)),
        ghost(t("crash-dismiss"), CrashUx::Dismiss),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);
    Some(
        container(r)
            .padding([theme::S3, theme::S7])
            .width(Length::Fill)
            .style(move |_| p.bar())
            .into(),
    )
}
