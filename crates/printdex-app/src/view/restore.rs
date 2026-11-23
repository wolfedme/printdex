//! Restore views: the "Restore from backup" wizard, the per-library
//! question after a restore, and the model's "Restore from backup…" panel.

use super::focus::button;
use super::focus::toggler;
use iced::widget::{Space, column, container, row, text};
use iced::{Alignment, Element, Length};
use printdex_core::ModelDetail;
use printdex_core::backup::{FileChange, SnapshotInfo};

use super::backup::{check, input, kind_label, sub_heading, when, when_full};
use super::settings::{setting, small_button};
use super::widgets::{self, icon_button, segmented};
use crate::app::{App, Message};
use crate::backup_ui::{Field, Kind, target_kinds};
use crate::format::size;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::restore_ui::{Relink, Rx, Step, relink_choices, to_restore};
use crate::theme::{self, Palette};

const WHEN_WIDTH: f32 = 170.0;

fn msg(r: Rx) -> Message {
    Message::Restore(r)
}

/// Whether a snapshot was made by this install (or one it was restored
/// from).
fn from_here(app: &App, s: &SnapshotInfo) -> bool {
    app.engine
        .as_ref()
        .is_some_and(|e| e.install_id().to_string() == s.host)
}

fn card<'a>(p: Palette, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(theme::S7)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

fn rx_field<'a>(
    p: Palette,
    label: String,
    ph: &str,
    value: &str,
    f: Field,
) -> Element<'a, Message> {
    let secure = matches!(f, Field::Secret | Field::Passphrase);
    input(p, label, ph, value, move |v| msg(Rx::Input(f, v)), secure)
}

/// Settings › Backup while restoring.
pub fn wizard(app: &App, p: Palette) -> Element<'_, Message> {
    let Some(w) = &app.restore.wizard else {
        return Space::new().into();
    };
    let mut col = column![].spacing(theme::S6);
    if let Some(e) = &w.error {
        col = col.push(
            row![
                icons::icon(Icon::Alert, theme::ICON, p.status.fail),
                text(e.clone()).size(theme::SIZE_BODY).color(p.n.ink),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center),
        );
    }
    let body: Element<'_, Message> = match w.step {
        Step::Target | Step::Finding => target_step(app, p, w),
        Step::Pick | Step::Restoring => pick_step(app, p, w),
        Step::Done => card(
            p,
            column![
                sub_heading(p, t("restore-done-title"), t("restore-done-body")),
                row![
                    Space::new().width(Length::Fill),
                    icon_button(
                        p,
                        Icon::Refresh,
                        t("restore-restart"),
                        Palette::button_primary,
                        Some(msg(Rx::Restart)),
                    ),
                ],
            ]
            .spacing(theme::S6),
        ),
    };
    col.push(body).into()
}

fn target_step<'a>(
    app: &'a App,
    p: Palette,
    w: &'a crate::restore_ui::Wizard,
) -> Element<'a, Message> {
    let f = &w.form;
    let kind = f.kind.unwrap_or(Kind::Folder);
    let kinds: Vec<_> = target_kinds(cfg!(windows))
        .into_iter()
        .map(|k| (k, None, Some(kind_label(k))))
        .collect();
    let mut place = column![
        sub_heading(p, t("restore-title"), t("restore-body")),
        segmented(p, &kinds, kind, |k| msg(Rx::Kind(k))),
    ]
    .spacing(theme::S4);
    place = match kind {
        Kind::Folder => place.push(
            row![
                container(rx_field(
                    p,
                    t("backup-field-folder"),
                    "/media/backup/printdex",
                    &f.path,
                    Field::Path
                ))
                .width(Length::Fill),
                small_button(p, t("backup-choose"), msg(Rx::PickFolder)),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center),
        ),
        Kind::S3 => place
            .push(rx_field(
                p,
                t("backup-field-endpoint"),
                "",
                &f.endpoint,
                Field::Endpoint,
            ))
            .push(rx_field(
                p,
                t("backup-field-bucket"),
                "",
                &f.bucket,
                Field::Bucket,
            ))
            .push(rx_field(
                p,
                t("backup-field-region"),
                "auto",
                &f.region,
                Field::Region,
            ))
            .push(rx_field(
                p,
                t("backup-field-prefix"),
                "",
                &f.prefix,
                Field::Prefix,
            ))
            .push(rx_field(
                p,
                t("backup-field-key-id"),
                "",
                &f.key_id,
                Field::KeyId,
            ))
            .push(rx_field(
                p,
                t("backup-field-secret-key"),
                "",
                &f.secret,
                Field::Secret,
            )),
        Kind::WebDav => place
            .push(rx_field(p, t("backup-field-url"), "", &f.url, Field::Url))
            .push(rx_field(
                p,
                t("backup-field-user"),
                "",
                &f.user,
                Field::User,
            ))
            .push(rx_field(
                p,
                t("backup-field-password"),
                "",
                &f.secret,
                Field::Secret,
            )),
        Kind::Sftp => place
            .push(rx_field(
                p,
                t("backup-field-host"),
                "",
                &f.host,
                Field::Host,
            ))
            .push(rx_field(
                p,
                t("backup-field-port"),
                "22",
                &f.port,
                Field::Port,
            ))
            .push(rx_field(
                p,
                t("backup-field-user"),
                "",
                &f.user,
                Field::User,
            ))
            .push(rx_field(
                p,
                t("backup-field-remote-folder"),
                "",
                &f.sftp_path,
                Field::SftpPath,
            ))
            .push(rx_field(
                p,
                t("backup-field-key-file"),
                &t("backup-key-agent"),
                &f.key_file,
                Field::KeyFile,
            )),
    };
    let encrypted = toggler(f.encrypted)
        .size(20)
        .style(move |_, status| p.toggler(status))
        .on_toggle(|on| msg(Rx::Encrypted(on)));
    place = place.push(widgets::hairline(p)).push(setting(
        p,
        t("restore-encrypted"),
        t("restore-encrypted-body"),
        encrypted.into(),
    ));
    if f.encrypted {
        place = place.push(rx_field(
            p,
            t("backup-field-passphrase"),
            "",
            &f.passphrase,
            Field::Passphrase,
        ));
    }
    place = place.push(check(p, t("backup-remember"), f.remember, |on| {
        msg(Rx::Remember(on))
    }));
    if !app.backup.persistent {
        place = place.push(
            text(t("backup-no-keyring"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    let finding = w.step == Step::Finding;
    place = place.push(
        row![
            Space::new().width(Length::Fill),
            icon_button(
                p,
                Icon::Close,
                t("settings-cancel"),
                Palette::button_secondary,
                Some(msg(Rx::Cancel)),
            ),
            icon_button(
                p,
                Icon::Search,
                if finding {
                    t("restore-finding")
                } else {
                    t("restore-find")
                },
                Palette::button_primary,
                w.can_find().then_some(msg(Rx::Find)),
            ),
        ]
        .spacing(theme::S3),
    );
    card(p, place.spacing(theme::S5))
}

fn pick_step<'a>(
    app: &'a App,
    p: Palette,
    w: &'a crate::restore_ui::Wizard,
) -> Element<'a, Message> {
    let mut list = column![].spacing(theme::S1);
    for s in &w.snapshots {
        let picked = w.picked.as_deref() == Some(s.id.as_str());
        let origin = if from_here(app, s) {
            t("restore-this-computer")
        } else {
            t("restore-other-computer")
        };
        list = list.push(
            button(
                row![
                    icons::icon(
                        if picked {
                            Icon::Success
                        } else {
                            Icon::NeverPrinted
                        },
                        theme::ICON,
                        if picked { p.t.accent_ink } else { p.n.ink_3 }
                    ),
                    text(when_full(s.time))
                        .size(theme::SIZE_BODY)
                        .font(theme::FONT_STRONG)
                        .width(WHEN_WIDTH),
                    text(origin)
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2)
                        .width(Length::Fill),
                    text(t_args(
                        "restore-snapshot-size",
                        &[
                            ("files", Arg::count(s.files)),
                            ("size", Arg::str(size(s.bytes))),
                        ],
                    ))
                    .size(theme::SIZE_DATA)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_2),
                ]
                .spacing(theme::S4)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([theme::S3, theme::S4])
            .style(move |_, status| p.rail_item(picked, status))
            .on_press(msg(Rx::Pick(s.id.clone()))),
        );
    }
    let restoring = w.step == Step::Restoring;
    let mut col = column![
        sub_heading(p, t("restore-pick-title"), t("restore-pick-body")),
        list,
    ]
    .spacing(theme::S5);
    let picked_other = w
        .snapshots
        .iter()
        .find(|s| w.picked.as_deref() == Some(s.id.as_str()))
        .is_some_and(|s| !from_here(app, s));
    if !app.libraries.is_empty() {
        col = col.push(warning(p, t("restore-replaces")));
        if picked_other {
            col = col.push(warning(p, t("restore-other-warning")));
        }
    }
    col = col.push(
        row![
            Space::new().width(Length::Fill),
            icon_button(
                p,
                Icon::Undo,
                t("backup-back"),
                Palette::button_secondary,
                (!restoring).then_some(msg(Rx::Back)),
            ),
            icon_button(
                p,
                Icon::Backup,
                if restoring {
                    t("restore-restoring")
                } else {
                    t("restore-go")
                },
                Palette::button_primary,
                (!restoring && w.picked.is_some()).then_some(msg(Rx::Restore)),
            ),
        ]
        .spacing(theme::S3),
    );
    card(p, col)
}

fn warning<'a>(p: Palette, s: String) -> Element<'a, Message> {
    row![
        icons::icon(Icon::Alert, theme::ICON, p.status.partial),
        text(s).size(theme::SIZE_CAPTION).color(p.n.ink_2),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center)
    .into()
}

/// Over the library after a restore: where each library lives now.
pub fn relink_banner(app: &App, p: Palette) -> Option<Element<'_, Message>> {
    if app.restore.pending.is_empty() {
        return None;
    }
    let mut col = column![].spacing(theme::S3);
    for pending in &app.restore.pending {
        let lib = &pending.library;
        let id = lib.id;
        let busy = app.restore.busy == Some(id);
        let mut r = row![
            icons::icon(Icon::Folder, theme::ICON, p.n.ink_2),
            column![
                text(t_args(
                    "restore-where",
                    &[("library", Arg::str(lib.display_name.clone()))]
                ))
                .size(theme::SIZE_BODY)
                .font(theme::FONT_STRONG)
                .color(p.n.ink),
                text(if busy {
                    t("restore-relinking")
                } else {
                    t_args(
                        "restore-where-body",
                        &[("path", Arg::str(lib.path.display().to_string()))],
                    )
                })
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_2),
            ]
            .spacing(theme::S1)
            .width(Length::Fill),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center);
        for choice in relink_choices(pending.snapshot.is_some()) {
            let (label, m, primary) = match choice {
                Relink::Here => (t("restore-here"), Rx::Here(id), true),
                Relink::FromBackup => (t("restore-files"), Rx::FromBackup(id), false),
                Relink::Skip => (t("restore-skip"), Rx::Skip(id), false),
            };
            r = r.push(
                button(text(label).size(theme::SIZE_CAPTION))
                    .padding([5.0, theme::S4])
                    .style(move |_, status| {
                        if primary {
                            p.button_primary(status)
                        } else {
                            p.button_secondary(status)
                        }
                    })
                    .on_press_maybe((!busy).then_some(msg(m))),
            );
        }
        col = col.push(r);
    }
    Some(
        container(col)
            .padding([theme::S4, theme::S7])
            .width(Length::Fill)
            .style(move |_| p.bar())
            .into(),
    )
}

/// Under the model header: "Restore from backup…".
pub fn model_panel<'a>(
    app: &'a App,
    p: Palette,
    d: &'a ModelDetail,
) -> Option<Element<'a, Message>> {
    let m = &app.restore.model;
    if m.model != Some(d.id) {
        return None;
    }
    let mut col = column![
        row![
            text(t("restore-model-title"))
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_BOLD)
                .color(p.n.ink)
                .width(Length::Fill),
            widgets::ghost_icon(p, Icon::Close, t("restore-close"), msg(Rx::CloseModel)),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(theme::S4);
    match &m.snapshots {
        None => {
            col = col.push(
                text(t("restore-model-loading"))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink_2),
            );
        }
        Some(snaps) if snaps.is_empty() => {
            col = col.push(
                text(m.error.clone().unwrap_or_else(|| t("restore-model-none")))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink_2),
            );
        }
        Some(snaps) => {
            let mut chips = row![].spacing(theme::S2);
            for s in snaps.iter().take(8) {
                let picked = m.picked.as_deref() == Some(s.id.as_str());
                chips = chips.push(
                    button(text(when(s.time)).size(theme::SIZE_CAPTION))
                        .padding([4.0, theme::S4])
                        .style(move |_, status| p.segment(picked, status))
                        .on_press(msg(Rx::PickModel(s.id.clone()))),
                );
            }
            col = col.push(chips.wrap().vertical_spacing(theme::S2));
            match &m.preview {
                None => {
                    col = col.push(
                        text(t("restore-model-comparing"))
                            .size(theme::SIZE_CAPTION)
                            .color(p.n.ink_2),
                    );
                }
                Some(preview) => {
                    let changed = to_restore(preview);
                    if changed.is_empty() {
                        col = col.push(
                            text(t("restore-model-same"))
                                .size(theme::SIZE_BODY)
                                .color(p.n.ink_2),
                        );
                    } else {
                        let mut files = column![].spacing(theme::S1);
                        for f in changed {
                            let (label, color) = match f.change {
                                FileChange::Missing => (t("restore-file-missing"), p.status.fail),
                                _ => (t("restore-file-changed"), p.status.partial),
                            };
                            files = files.push(
                                row![
                                    text(f.rel.clone())
                                        .size(theme::SIZE_DATA)
                                        .font(theme::FONT_DATA)
                                        .color(p.n.ink)
                                        .width(Length::Fill),
                                    text(label)
                                        .size(theme::SIZE_CAPTION)
                                        .font(theme::FONT_STRONG)
                                        .color(color),
                                ]
                                .spacing(theme::S4),
                            );
                        }
                        col = col.push(files).push(
                            text(t("restore-model-trash"))
                                .size(theme::SIZE_CAPTION)
                                .color(p.n.ink_3),
                        );
                    }
                    if let Some(e) = &m.error {
                        col = col.push(
                            text(e.clone())
                                .size(theme::SIZE_CAPTION)
                                .color(p.status.fail),
                        );
                    }
                    if let Some(n) = m.done {
                        col = col.push(
                            text(t_args(
                                "restore-model-done",
                                &[("count", Arg::count(n as u64))],
                            ))
                            .size(theme::SIZE_CAPTION)
                            .color(p.status.ok),
                        );
                    }
                    let can = !m.busy && !to_restore(preview).is_empty();
                    col = col.push(
                        row![
                            Space::new().width(Length::Fill),
                            icon_button(
                                p,
                                Icon::FolderOpen,
                                t("restore-model-elsewhere"),
                                Palette::button_secondary,
                                (!m.busy).then_some(msg(Rx::RestoreElsewhere)),
                            ),
                            icon_button(
                                p,
                                Icon::Undo,
                                t("restore-model-here"),
                                Palette::button_primary,
                                can.then_some(msg(Rx::RestoreHere)),
                            ),
                        ]
                        .spacing(theme::S3),
                    );
                }
            }
        }
    }
    Some(
        container(col)
            .padding([theme::S5, theme::S7])
            .width(Length::Fill)
            .style(move |_| p.bar())
            .into(),
    )
}
