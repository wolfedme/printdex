//! File action views: the confirm dialogs, the undo toast and the
//! selection bar.

use super::focus::button;
use iced::widget::{Space, column, container, mouse_area, opaque, row, text, text_input};
use iced::{Alignment, Element, Length};

use printdex_core::Material;

use super::widgets::icon_button;
use crate::app::{App, Message};
use crate::files::{Dialog, Fx, name_list, rename_field_id, toast_text};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

const DIALOG_WIDTH: f32 = 440.0;
const TOAST_WIDTH: f32 = 420.0;

fn small<'a>(p: Palette, label: String, msg: Option<Message>) -> Element<'a, Message> {
    button(text(label).size(theme::SIZE_CAPTION))
        .padding([5.0, theme::S4])
        .style(move |_, status| p.button_secondary(status))
        .on_press_maybe(msg)
        .into()
}

/// The open dialog over a dimmed window, if any.
pub fn dialog(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    let d = app.files.dialog.as_ref()?;
    let busy = app.files.busy;
    let (icon, title, body, confirm, danger, error): (_, _, Element<'_, Message>, _, _, _) = match d
    {
        Dialog::Rename { name, error, .. } => (
            Icon::Rename,
            t("files-rename-title"),
            text_input("", name)
                .id(rename_field_id())
                .on_input(|s| Message::Files(Fx::RenameInput(s)))
                .on_submit(Message::Files(Fx::Confirm))
                .size(theme::SIZE_BODY)
                .padding([7.0, theme::S4])
                .style(move |_, status| p.search_field(status))
                .into(),
            t("files-rename-yes"),
            false,
            error,
        ),
        Dialog::Move {
            names,
            label,
            error,
            ..
        } => (
            Icon::Move,
            t_args(
                "files-move-title",
                &[
                    ("count", Arg::count(names.len() as u64)),
                    ("where", Arg::str(label.clone())),
                ],
            ),
            text(name_list(names))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2)
                .into(),
            t("files-move-yes"),
            false,
            error,
        ),
        Dialog::Trash { names, error, .. } => (
            Icon::Remove,
            t_args(
                "files-trash-title",
                &[("count", Arg::count(names.len() as u64))],
            ),
            column![
                text(name_list(names))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink_2),
                text(t("files-trash-body"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_3),
            ]
            .spacing(theme::S3)
            .into(),
            t("files-trash-yes"),
            true,
            error,
        ),
    };
    let can_confirm = !busy && !matches!(d, Dialog::Move { error: Some(_), .. });
    let confirm_button = button(
        text(confirm)
            .size(theme::SIZE_CAPTION)
            .font(theme::FONT_STRONG),
    )
    .padding([5.0, theme::S4])
    .style(move |_, status| {
        if danger {
            p.button_danger(status)
        } else {
            p.button_primary(status)
        }
    })
    .on_press_maybe(can_confirm.then_some(Message::Files(Fx::Confirm)));
    let mut col = column![
        row![
            icons::icon(icon, theme::ICON, p.n.ink_2),
            text(title)
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_BOLD)
                .color(p.n.ink),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
        body,
    ]
    .spacing(theme::S5);
    if let Some(e) = error {
        col = col.push(
            row![
                icons::icon(Icon::Alert, theme::ICON_DENSE, p.status.fail),
                text(e.clone())
                    .size(theme::SIZE_CAPTION)
                    .color(p.status.fail),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center),
        );
    }
    col = col.push(
        row![
            Space::new().width(Length::Fill),
            small(p, t("settings-cancel"), Some(Message::Files(Fx::Cancel))),
            confirm_button,
        ]
        .spacing(theme::S3),
    );
    let card = container(col)
        .padding(theme::S7)
        .width(DIALOG_WIDTH)
        .style(move |_| p.dialog());
    Some(opaque(
        mouse_area(
            container(opaque(card))
                .center(Length::Fill)
                .style(move |_| p.scrim()),
        )
        .on_press(Message::Files(Fx::Cancel)),
    ))
}

/// The undo toast (or a short note), bottom centre.
pub fn toast(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    let (content, ok): (Element<'_, Message>, bool) = match (&app.files.toast, &app.files.note) {
        (_, Some(note)) => (text(note.clone()).size(theme::SIZE_CAPTION).into(), false),
        (Some((op, _)), None) => {
            let mut r = row![
                text(toast_text(op))
                    .size(theme::SIZE_CAPTION)
                    .width(Length::Fill),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center);
            if op.undoable {
                r = r.push(
                    button(
                        row![
                            icons::icon(Icon::Undo, theme::ICON_CHIP, p.t.accent_ink),
                            text(t("files-undo"))
                                .size(theme::SIZE_CAPTION)
                                .font(theme::FONT_STRONG)
                                .color(p.t.accent_ink),
                        ]
                        .spacing(theme::S2)
                        .align_y(Alignment::Center),
                    )
                    .padding([3.0, theme::S3])
                    .style(move |_, status| p.button_ghost(status))
                    .on_press(Message::Files(Fx::Undo(op.id))),
                );
            }
            (r.into(), true)
        }
        (None, None) => (
            text(app.files.info.clone()?)
                .size(theme::SIZE_CAPTION)
                .into(),
            true,
        ),
    };
    let card = container(
        row![
            icons::icon(
                if ok { Icon::Success } else { Icon::Alert },
                theme::ICON_DENSE,
                if ok { p.status.ok } else { p.status.fail }
            ),
            content,
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    )
    .padding([theme::S4, theme::S5])
    .width(TOAST_WIDTH)
    .style(move |_| p.dialog());
    Some(
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End)
            .padding(iced::Padding {
                // Clear of the selection bar while it shows.
                bottom: if app.files.picked.len() >= 2 {
                    theme::S8 + SELECTION_BAR_HEIGHT
                } else {
                    theme::S8
                },
                ..iced::Padding::new(theme::S8)
            })
            .into(),
    )
}

/// Height of the selection bar: a 32px control with 6px above and below.
const SELECTION_BAR_HEIGHT: f32 = theme::CONTROL_HEIGHT + 2.0 * theme::S3;

/// "3 selected" with the actions, while more than one model is picked.
pub fn selection_bar(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    let n = app.files.picked.len();
    if n < 2 {
        return None;
    }
    Some(
        container(
            row![
                text(t_args("files-selected", &[("count", Arg::count(n as u64))]))
                    .size(theme::SIZE_BODY)
                    .font(theme::FONT_STRONG)
                    .color(p.n.ink),
                Space::new().width(Length::Fill),
                material_menu(app, p),
                icon_button(
                    p,
                    Icon::Move,
                    t("files-move"),
                    Palette::button_secondary,
                    Some(Message::Files(Fx::AskMove))
                ),
                icon_button(
                    p,
                    Icon::Remove,
                    t("files-trash"),
                    Palette::button_secondary,
                    Some(Message::Files(Fx::AskTrash))
                ),
                icon_button(
                    p,
                    Icon::Close,
                    t("files-clear"),
                    Palette::button_ghost,
                    Some(Message::Files(Fx::ClearSelection))
                ),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center),
        )
        .padding([theme::S3, theme::S7])
        .width(Length::Fill)
        .style(move |_| p.bar())
        .into(),
    )
}

/// Width of the "Set material" menu.
const MATERIAL_MENU_WIDTH: f32 = 200.0;

/// "Set material" with its menu: the known materials, then "No material".
fn material_menu(app: &App, p: Palette) -> Element<'_, Message> {
    let anchor = icon_button(
        p,
        Icon::Material,
        t("files-set-material"),
        Palette::button_secondary,
        Some(Message::Files(Fx::ToggleMaterialMenu)),
    );
    let menu = app.files.material_menu.then(|| {
        let item = |label: String, msg: Fx| {
            button(text(label).size(theme::SIZE_BODY).width(Length::Fill))
                .width(Length::Fill)
                .padding([4.0, theme::S4])
                .style(move |_, status| p.button_ghost(status))
                .on_press(Message::Files(msg))
        };
        let mut menu = column![].spacing(theme::S1);
        for m in Material::KNOWN {
            menu = menu.push(item(m.label().to_owned(), Fx::SetMaterial(Some(m))));
        }
        menu = menu
            .push(super::widgets::hairline(p))
            .push(item(t("files-clear-material"), Fx::SetMaterial(None)));
        container(menu)
            .padding(theme::S2)
            .width(Length::Fill)
            .style(move |_| p.dialog())
            .into()
    });
    super::dropdown::dropdown(anchor, menu, MATERIAL_MENU_WIDTH)
        .on_dismiss(Message::Files(Fx::ToggleMaterialMenu))
        .into()
}
