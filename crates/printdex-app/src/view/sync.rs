//! Settings › Sync, the sidebar's sync line with its popover, and the
//! model view's "Edited elsewhere" list.

use iced::widget::{Space, button, column, container, row, text, text_input, toggler};
use iced::{Alignment, Element, Length};
use printdex_core::ModelId;
use printdex_core::db::SpaceKind;
use printdex_core::sync::SpaceInfo;

use super::backup::{
    check, error_banner_with, input, kind_label, sub_heading, target_fields, when,
};
use super::settings::{heading, setting, small_button};
use super::widgets::{self, icon_button, segmented};
use crate::app::{App, Message};
use crate::backup_ui::{Field, Kind, needs_secret, target_kinds};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::sync_ui::{
    AddSpace, LinkDialog, Place, Sx, Tone, ago, can_forget, edit_label, edit_text, sync_status_text,
};
use crate::theme::{self, Palette};

/// Width of the sidebar popover.
const POPOVER_WIDTH: f32 = 208.0;

fn sx(m: Sx) -> Message {
    Message::Sync(m)
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// The sidebar footer line and its popover, while a space is set up.
pub fn status<'a>(app: &'a App, p: Palette) -> Option<Element<'a, Message>> {
    let s = &app.sync;
    if !s.shown() {
        return None;
    }
    let now = now_ms();
    let (label, tone) = if s.enabled && !s.running.is_empty() {
        (t("sync-status-syncing"), Tone::Busy)
    } else {
        sync_status_text(s.enabled, &s.spaces, now)
    };
    let (color, icon) = match tone {
        Tone::Quiet => (p.n.on_rail_2, Icon::Sync),
        Tone::Busy => (p.n.on_rail, Icon::Refresh),
        Tone::Attention => (p.status.fail, Icon::Alert),
    };
    let line = button(
        row![
            icons::icon(icon, theme::ICON_DENSE, color),
            text(label)
                .size(theme::SIZE_CAPTION)
                .color(color)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([4, 8])
    .style(move |_, status| p.rail_item(false, status))
    .on_press(sx(Sx::TogglePopover));

    let menu = s.popover.then(|| popover(app, p, now));
    Some(
        super::dropdown::dropdown(line, menu, POPOVER_WIDTH)
            .on_dismiss(sx(Sx::ClosePopover))
            .into(),
    )
}

fn popover<'a>(app: &'a App, p: Palette, now: i64) -> Element<'a, Message> {
    let s = &app.sync;
    let mut col = column![].spacing(theme::S3);
    for space in &s.spaces {
        let (state, color) = space_state(space, s.running.contains(&space.id), now, p);
        col = col.push(
            column![
                text(space.name.clone())
                    .size(theme::SIZE_CAPTION)
                    .font(theme::FONT_STRONG)
                    .color(p.n.ink),
                text(state).size(theme::SIZE_CAPTION).color(color),
            ]
            .spacing(2),
        );
    }
    let queued = s.queued as u64;
    col = col.push(widgets::hairline(p)).push(
        text(t_args(
            "sync-pop-queued",
            &[
                ("count", Arg::count(queued)),
                ("n", Arg::str(queued.to_string())),
            ],
        ))
        .size(theme::SIZE_CAPTION)
        .color(p.n.ink_2),
    );
    let can_sync = s.enabled && s.running.is_empty();
    col = col.push(
        row![
            icon_button(
                p,
                Icon::Refresh,
                t("sync-now"),
                Palette::button_secondary,
                can_sync.then_some(sx(Sx::SyncNow)),
            ),
            widgets::ghost_icon(p, Icon::Settings, t("sync-pop-settings"), sx(Sx::Open)),
        ]
        .spacing(theme::S2)
        .align_y(Alignment::Center),
    );
    container(col)
        .padding([theme::S4, theme::S5])
        .width(Length::Fill)
        .style(move |_| p.dialog())
        .into()
}

/// "Synced 2 min ago", "Locked", "Last try failed: …", with its colour.
fn space_state(space: &SpaceInfo, running: bool, now: i64, p: Palette) -> (String, iced::Color) {
    if running {
        return (t("sync-syncing"), p.t.accent_ink);
    }
    if space.locked {
        return (t("sync-status-locked"), p.status.fail);
    }
    if let Some(e) = &space.last_error {
        return (
            t_args("sync-last-error", &[("error", Arg::str(e.clone()))]),
            p.status.fail,
        );
    }
    match space.last_sync_at {
        Some(at) => (
            t_args("sync-last", &[("ago", Arg::str(ago(at, now)))]),
            p.n.ink_2,
        ),
        None => (t("sync-last-never"), p.n.ink_2),
    }
}

/// Settings › Sync.
pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let s = &app.sync;
    let mut col = column![heading(p, t("settings-sync"), t("sync-body"))].spacing(theme::S6);
    if let Some(e) = &s.error {
        col = col.push(error_banner_with(p, e.clone(), sx(Sx::DismissError)));
    }
    if !s.loaded {
        return col
            .push(
                text(t("sync-loading"))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink_2),
            )
            .into();
    }
    if let Some(a) = &s.add {
        return col.push(add_form(app, p, a)).into();
    }
    if let Some(d) = &s.link {
        return col.push(link_dialog(app, p, d)).into();
    }

    // On/off and this device's name.
    let on = toggler(s.enabled)
        .size(20)
        .style(move |_, status| p.toggler(status))
        .on_toggle(|on| sx(Sx::SetEnabled(on)));
    let changed = s.device.trim() != s.device_saved && !s.device.trim().is_empty();
    let device = row![
        text_input("", &s.device)
            .on_input(|v| sx(Sx::DeviceInput(v)))
            .on_submit(sx(Sx::SaveDevice))
            .size(theme::SIZE_BODY)
            .padding([7.0, theme::S4])
            .width(220)
            .style(move |_, status| p.search_field(status)),
        button(text(t("sync-device-save")).size(theme::SIZE_CAPTION))
            .padding([4.0, theme::S4])
            .style(move |_, status| p.button_secondary(status))
            .on_press_maybe(changed.then_some(sx(Sx::SaveDevice))),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);
    col = col.push(
        column![
            setting(p, t("sync-on"), t("sync-on-body"), on.into()),
            Space::new().height(theme::S5),
            setting(p, t("sync-device"), t("sync-device-body"), device.into()),
        ]
        .spacing(0),
    );

    if s.spaces.is_empty() {
        return col.push(empty(p)).into();
    }

    let mut spaces =
        column![sub_heading(p, t("sync-spaces"), t("sync-spaces-body"))].spacing(theme::S4);
    let now = now_ms();
    for space in &s.spaces {
        spaces = spaces.push(space_card(app, p, space, now));
    }
    let can_sync = s.enabled && s.running.is_empty();
    spaces = spaces.push(
        row![
            icon_button(
                p,
                Icon::Plus,
                t("sync-add"),
                Palette::button_secondary,
                Some(sx(Sx::StartAdd)),
            ),
            Space::new().width(Length::Fill),
            icon_button(
                p,
                Icon::Refresh,
                if s.running.is_empty() {
                    t("sync-now")
                } else {
                    t("sync-syncing")
                },
                Palette::button_primary,
                can_sync.then_some(sx(Sx::SyncNow)),
            ),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    );
    col.push(spaces).into()
}

fn empty<'a>(p: Palette) -> Element<'a, Message> {
    container(
        column![
            icons::icon(Icon::Sync, 28.0, p.n.ink_2),
            text(t("sync-empty-title"))
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_BOLD)
                .color(p.n.ink),
            text(t("sync-empty-body"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2),
            Space::new().height(theme::S2),
            icon_button(
                p,
                Icon::Plus,
                t("sync-add"),
                Palette::button_primary,
                Some(sx(Sx::StartAdd)),
            ),
        ]
        .spacing(theme::S3),
    )
    .padding(theme::S7)
    .width(Length::Fill)
    .style(move |_| p.pane(false, false))
    .into()
}

fn library_name(app: &App, id: printdex_core::LibraryId) -> String {
    app.library(id)
        .map(|l| l.display_name.clone())
        .unwrap_or_else(|| t("backup-removed-library"))
}

fn space_card<'a>(
    app: &'a App,
    p: Palette,
    space: &'a SpaceInfo,
    now: i64,
) -> Element<'a, Message> {
    let s = &app.sync;
    let open = s.expanded.contains(&space.id);
    let id = space.id;
    let (state, state_color) = space_state(space, s.running.contains(&id), now, p);
    let mut tags = Vec::new();
    if space.kind == SpaceKind::Library {
        tags.push(t("sync-kind-label-library"));
    }
    if space.encrypted {
        tags.push(t("sync-encrypted"));
    }
    tags.push(space.location.clone());
    let head = button(
        row![
            icons::icon(
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::Chevron
                },
                theme::ICON_DENSE,
                p.n.ink_2
            ),
            column![
                text(space.name.clone())
                    .size(theme::SIZE_BODY)
                    .font(theme::FONT_STRONG)
                    .color(p.n.ink),
                text(tags.join(" · "))
                    .size(theme::SIZE_DATA)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_2)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(2)
            .width(Length::Fill),
            row![
                if space.locked {
                    icons::icon(Icon::Lock, theme::ICON_DENSE, p.status.fail)
                } else {
                    icons::icon(Icon::Sync, theme::ICON_DENSE, state_color)
                },
                text(state)
                    .size(theme::SIZE_CAPTION)
                    .color(state_color)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(theme::S2)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([theme::S4, theme::S5])
    .style(move |_, status| p.button_ghost(status))
    .on_press(sx(Sx::Expand(id)));

    let mut card = column![head].spacing(0);
    if open || space.locked {
        card = card.push(space_body(app, p, space, now));
    }
    container(card)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

fn label<'a>(p: Palette, s: String) -> Element<'a, Message> {
    text(s)
        .size(theme::SIZE_CAPTION)
        .font(theme::FONT_STRONG)
        .color(p.n.ink_2)
        .width(90)
        .into()
}

fn space_body<'a>(
    app: &'a App,
    p: Palette,
    space: &'a SpaceInfo,
    now: i64,
) -> Element<'a, Message> {
    let s = &app.sync;
    let id = space.id;
    let mut col = column![widgets::hairline(p)].spacing(theme::S4);

    // Locked: the passphrase prompt first.
    if space.locked {
        col = col.push(unlock(app, p, space));
    }

    // Libraries.
    let mut libs = column![].spacing(theme::S2);
    if space.libraries.is_empty() {
        libs = libs.push(
            text(t("sync-libraries-none"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    for (lib, _) in &space.libraries {
        let lib = *lib;
        let mut r = row![
            icons::icon(Icon::Folder, theme::ICON_DENSE, p.n.ink_2),
            text(library_name(app, lib))
                .size(theme::SIZE_BODY)
                .color(p.n.ink)
                .width(Length::Fill),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center);
        if space.kind == SpaceKind::Target {
            r = r.push(small_button(p, t("sync-unlink"), sx(Sx::Unlink(id, lib))));
        }
        libs = libs.push(r);
    }
    if space.kind == SpaceKind::Target {
        let unlinked = app
            .libraries
            .iter()
            .any(|l| !space.libraries.iter().any(|(id, _)| *id == l.id));
        if unlinked && !space.locked {
            libs = libs.push(
                row![small_button(p, t("sync-link"), sx(Sx::StartLink(id, None)))]
                    .spacing(theme::S3),
            );
        }
    }
    col =
        col.push(row![label(p, t("sync-libraries")), libs.width(Length::Fill)].spacing(theme::S4));

    // Devices.
    let mut devices = column![].spacing(theme::S2);
    let mine = s.me;
    devices = devices.push(
        row![
            icons::icon(Icon::Device, theme::ICON_DENSE, p.t.accent_ink),
            text(t_args(
                "sync-device-this",
                &[("name", Arg::str(s.device_saved.clone()))]
            ))
            .size(theme::SIZE_BODY)
            .color(p.n.ink),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    );
    let mut any_old = false;
    for d in space.devices.iter().filter(|d| Some(d.install) != mine) {
        let forget = can_forget(d, mine, now);
        any_old |= forget;
        let confirming = s.confirm_forget == Some((id, d.install));
        let mut r = row![
            icons::icon(Icon::Device, theme::ICON_DENSE, p.n.ink_2),
            text(t_args(
                "sync-device-seen",
                &[
                    ("name", Arg::str(d.name.clone())),
                    ("ago", Arg::str(ago(d.last_seen, now))),
                ]
            ))
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center);
        if forget && !confirming {
            r = r.push(small_button(
                p,
                t("sync-forget"),
                sx(Sx::AskForget(id, d.install)),
            ));
        }
        devices = devices.push(r);
        if confirming {
            devices = devices.push(confirm(
                p,
                t_args("sync-forget-confirm", &[("name", Arg::str(d.name.clone()))]),
                t("sync-forget-yes"),
                sx(Sx::CancelForget),
                sx(Sx::ConfirmForget),
            ));
        }
    }
    if !any_old && space.devices.iter().any(|d| Some(d.install) != mine) {
        devices = devices.push(
            text(t("sync-forget-hint"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    col =
        col.push(row![label(p, t("sync-devices")), devices.width(Length::Fill)].spacing(theme::S4));

    // Models that aren't here.
    if space.parked > 0 {
        let n = space.parked as u64;
        let showing = matches!(&s.parked, Some((sid, _)) if *sid == id);
        let mut parked = column![
            row![
                icons::icon(Icon::Info, theme::ICON_DENSE, p.n.ink_2),
                text(t_args(
                    "sync-parked",
                    &[("count", Arg::count(n)), ("n", Arg::str(n.to_string()))]
                ))
                .size(theme::SIZE_BODY)
                .color(p.n.ink)
                .width(Length::Fill),
                small_button(
                    p,
                    if showing {
                        t("sync-parked-hide")
                    } else {
                        t("sync-parked-show")
                    },
                    if showing {
                        sx(Sx::HideParked)
                    } else {
                        sx(Sx::ShowParked(id))
                    }
                ),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center)
        ]
        .spacing(theme::S2);
        if let Some((_, list)) = s.parked.as_ref().filter(|(sid, _)| *sid == id) {
            parked = parked.push(
                text(t("sync-parked-body"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_3),
            );
            for m in list.iter().take(50) {
                parked = parked.push(
                    row![
                        text(format!("/{}", m.path))
                            .size(theme::SIZE_DATA)
                            .font(theme::FONT_DATA)
                            .color(p.n.ink)
                            .width(Length::Fill),
                        text(t_args(
                            "sync-parked-from",
                            &[("device", Arg::str(m.device.clone()))]
                        ))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2),
                    ]
                    .spacing(theme::S3),
                );
            }
        }
        col = col.push(parked);
    }

    // Leave.
    if s.confirm_remove == Some(id) {
        col = col.push(confirm(
            p,
            t_args(
                "sync-remove-confirm",
                &[("space", Arg::str(space.name.clone()))],
            ),
            t("sync-remove-yes"),
            sx(Sx::CancelRemove),
            sx(Sx::ConfirmRemove),
        ));
    } else {
        col = col.push(
            row![
                Space::new().width(Length::Fill),
                small_button(p, t("sync-remove"), sx(Sx::AskRemove(id))),
            ]
            .spacing(theme::S3),
        );
    }
    container(col)
        .padding(iced::Padding {
            top: 0.0,
            right: theme::S5,
            bottom: theme::S5,
            left: theme::S5,
        })
        .width(Length::Fill)
        .into()
}

fn confirm<'a>(
    p: Palette,
    question: String,
    yes: String,
    cancel: Message,
    ok: Message,
) -> Element<'a, Message> {
    container(
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(question)
                .size(theme::SIZE_BODY)
                .color(p.n.ink)
                .width(Length::Fill),
            button(text(t("settings-cancel")).size(theme::SIZE_CAPTION))
                .padding([5.0, theme::S4])
                .style(move |_, status| p.button_secondary(status))
                .on_press(cancel),
            button(text(yes).size(theme::SIZE_CAPTION).font(theme::FONT_STRONG))
                .padding([5.0, theme::S4])
                .style(move |_, status| p.button_danger(status))
                .on_press(ok),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .padding([theme::S4, theme::S5])
    .style(move |_| p.error_line())
    .into()
}

fn unlock<'a>(app: &'a App, p: Palette, space: &'a SpaceInfo) -> Element<'a, Message> {
    let s = &app.sync;
    let Some(u) = s.unlock.as_ref().filter(|u| u.space == space.id) else {
        return row![
            icons::icon(Icon::Lock, theme::ICON, p.status.fail),
            text(t_args(
                "sync-unlock-body",
                &[("space", Arg::str(space.name.clone()))]
            ))
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
            small_button(p, t("sync-unlock"), sx(Sx::StartUnlock(space.id))),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center)
        .into();
    };
    let mut col = column![
        input(
            p,
            t("backup-field-passphrase"),
            "",
            &u.passphrase,
            |v| sx(Sx::UnlockInput(v)),
            true
        ),
        check(p, t("backup-remember"), u.remember, |on| {
            sx(Sx::UnlockRemember(on))
        }),
    ]
    .spacing(theme::S4);
    if let Some(e) = &u.error {
        col = col.push(
            text(e.clone())
                .size(theme::SIZE_CAPTION)
                .color(p.status.fail),
        );
    }
    col.push(
        row![
            Space::new().width(Length::Fill),
            small_button(p, t("settings-cancel"), sx(Sx::CancelUnlock)),
            icon_button(
                p,
                Icon::Lock,
                t("sync-unlock-confirm"),
                Palette::button_primary,
                (!u.busy && !u.passphrase.is_empty()).then_some(sx(Sx::ConfirmUnlock)),
            ),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    )
    .into()
}

fn pass_field<'a>(p: Palette, label: String, value: &str, f: Field) -> Element<'a, Message> {
    input(p, label, "", value, move |v| sx(Sx::Input(f, v)), true)
}

fn add_form<'a>(app: &'a App, p: Palette, a: &'a AddSpace) -> Element<'a, Message> {
    let places = [
        (Place::Library, None, Some(t("sync-kind-library"))),
        (Place::Target, None, Some(t("sync-kind-target"))),
    ];
    let mut place = column![
        sub_heading(
            p,
            t("sync-where"),
            match a.place {
                Place::Library => t("sync-kind-library-body"),
                Place::Target => t("sync-kind-target-body"),
            }
        ),
        segmented(p, &places, a.place, |pl| sx(Sx::Place(pl))),
    ]
    .spacing(theme::S4);

    // The library.
    let libs: Vec<_> = app
        .libraries
        .iter()
        .map(|l| (l.id, None, Some(l.display_name.clone())))
        .collect();
    if let Some(sel) = a.library.or_else(|| app.libraries.first().map(|l| l.id)) {
        place = place.push(
            row![
                text(match a.place {
                    Place::Library => t("sync-library"),
                    Place::Target => t("sync-library-first"),
                })
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2)
                .width(160),
                segmented(p, &libs, sel, |l| sx(Sx::AddLibrary(l))),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center),
        );
    }

    if a.place == Place::Target {
        let kind = a.form.kind.unwrap_or(Kind::Folder);
        let kinds: Vec<_> = target_kinds(cfg!(windows))
            .into_iter()
            .map(|k| (k, None, Some(kind_label(k))))
            .collect();
        place = place
            .push(segmented(p, &kinds, kind, |k| sx(Sx::Kind(k))))
            .push(target_fields(
                p,
                &a.form,
                |f, v| Message::Sync(Sx::Input(f, v)),
                sx(Sx::PickFolder),
                sx(Sx::PickKeyFile),
                "~/Nextcloud/printdex-sync",
            ));
    }

    // Encryption.
    let f = &a.form;
    let encrypt = toggler(f.encrypted)
        .size(20)
        .style(move |_, status| p.toggler(status))
        .on_toggle(|on| sx(Sx::Encrypted(on)));
    let mut lock = column![setting(
        p,
        t("sync-encrypt"),
        match a.place {
            Place::Library => t("sync-encrypt-body-library"),
            Place::Target => t("sync-encrypt-body-target"),
        },
        encrypt.into()
    )]
    .spacing(theme::S4);
    if f.encrypted {
        if a.joining {
            lock = lock.push(
                text(t("sync-joining"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_2),
            );
        }
        lock = lock.push(pass_field(
            p,
            t("backup-field-passphrase"),
            &f.passphrase,
            Field::Passphrase,
        ));
        if !a.joining {
            lock = lock
                .push(pass_field(
                    p,
                    t("backup-field-passphrase2"),
                    &f.passphrase2,
                    Field::Passphrase2,
                ))
                .push(check(p, t("sync-ack"), f.ack, |on| sx(Sx::Ack(on))));
        }
    }
    let remote_secret = a.place == Place::Target && f.target().is_some_and(|t| needs_secret(&t));
    if a.place == Place::Target && (f.encrypted || remote_secret) {
        lock = lock.push(check(p, t("backup-remember"), f.remember, |on| {
            sx(Sx::Remember(on))
        }));
    }
    if let Some(e) = &a.error {
        lock = lock.push(
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

    let blocker = a.save_blocker().unwrap_or_default();
    let actions = row![
        text(blocker)
            .size(theme::SIZE_CAPTION)
            .color(p.n.ink_3)
            .width(Length::Fill),
        icon_button(
            p,
            Icon::Close,
            t("settings-cancel"),
            Palette::button_secondary,
            Some(sx(Sx::CancelAdd)),
        ),
        icon_button(
            p,
            Icon::Check,
            if a.saving {
                t("sync-saving")
            } else {
                t("sync-save")
            },
            Palette::button_primary,
            a.can_save().then_some(sx(Sx::Save)),
        ),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);

    container(column![place, widgets::hairline(p), lock, actions].spacing(theme::S7))
        .padding(theme::S7)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

fn choice_row<'a>(
    p: Palette,
    selected: bool,
    title: String,
    caption: String,
    msg: Message,
) -> Element<'a, Message> {
    button(
        row![
            icons::icon(
                if selected {
                    Icon::Success
                } else {
                    Icon::NeverPrinted
                },
                theme::ICON,
                if selected { p.t.accent_ink } else { p.n.ink_3 }
            ),
            column![
                text(title)
                    .size(theme::SIZE_BODY)
                    .font(if selected {
                        theme::FONT_STRONG
                    } else {
                        theme::FONT_BODY
                    })
                    .color(p.n.ink),
                text(caption).size(theme::SIZE_CAPTION).color(p.n.ink_2),
            ]
            .spacing(2),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([theme::S3, theme::S4])
    .style(move |_, status| p.rail_item(selected, status))
    .on_press(msg)
    .into()
}

fn link_dialog<'a>(app: &'a App, p: Palette, d: &'a LinkDialog) -> Element<'a, Message> {
    let space = app.sync.space(d.space);
    let linked: Vec<_> = space
        .map(|s| s.libraries.iter().map(|(l, _)| *l).collect())
        .unwrap_or_default();
    let locals: Vec<_> = app
        .libraries
        .iter()
        .filter(|l| !linked.contains(&l.id))
        .map(|l| (l.id, None, Some(l.display_name.clone())))
        .collect();
    let mut col =
        column![sub_heading(p, t("sync-link-title"), t("sync-link-body"))].spacing(theme::S5);
    if locals.is_empty() {
        col = col.push(
            text(t("sync-link-none"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2),
        );
    } else {
        let sel = d.library.unwrap_or(locals[0].0);
        col = col.push(
            row![
                text(t("sync-link-local"))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink_2)
                    .width(160),
                segmented(p, &locals, sel, |l| sx(Sx::LinkLibrary(l))),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center),
        );
        let mut remotes = column![
            text(t("sync-link-remote"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2)
        ]
        .spacing(theme::S2);
        for r in d.open_remotes() {
            let mut caption = t_args(
                "sync-link-from",
                &[("device", Arg::str(r.created_by.clone()))],
            );
            if d.library.is_some() && r.suggested == d.library {
                caption = format!("{caption} · {}", t("sync-link-suggested"));
            }
            remotes = remotes.push(choice_row(
                p,
                d.choice == Some(r.id),
                r.name.clone(),
                caption,
                sx(Sx::LinkChoice(Some(r.id))),
            ));
        }
        remotes = remotes.push(choice_row(
            p,
            d.choice.is_none(),
            t("sync-link-new"),
            String::new(),
            sx(Sx::LinkChoice(None)),
        ));
        col = col.push(remotes);
    }
    let can = d.library.is_some() && !d.busy;
    col = col.push(
        row![
            Space::new().width(Length::Fill),
            icon_button(
                p,
                Icon::Close,
                t("settings-cancel"),
                Palette::button_secondary,
                Some(sx(Sx::CancelLink)),
            ),
            icon_button(
                p,
                Icon::Link,
                if d.busy {
                    t("sync-link-linking")
                } else {
                    t("sync-link-confirm")
                },
                Palette::button_primary,
                can.then_some(sx(Sx::ConfirmLink)),
            ),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    );
    container(col)
        .padding(theme::S7)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

/// "Edited elsewhere" under the notes of model `id`, when a concurrent
/// edit lost.
pub fn edited_elsewhere<'a>(app: &'a App, p: Palette, id: ModelId) -> Option<Element<'a, Message>> {
    let m = &app.sync.model;
    if m.id != Some(id) || m.history.is_empty() {
        return None;
    }
    let mut list = column![].spacing(theme::S4);
    for (i, e) in m.history.iter().enumerate() {
        if i > 0 {
            list = list.push(widgets::hairline(p));
        }
        let body = edit_text(e);
        list = list.push(
            column![
                row![
                    text(t_args(
                        "sync-edited-meta",
                        &[
                            ("field", Arg::str(edit_label(&e.key))),
                            ("device", Arg::str(e.device.clone())),
                            ("time", Arg::str(when(e.at))),
                        ]
                    ))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_2)
                    .width(Length::Fill),
                    small_button(p, t("sync-edited-restore"), sx(Sx::RestoreEdit(e.id))),
                    small_button(p, t("sync-edited-copy"), sx(Sx::CopyEdit(body.clone()))),
                    small_button(p, t("sync-edited-dismiss"), sx(Sx::DismissEdit(e.id))),
                ]
                .spacing(theme::S2)
                .align_y(Alignment::Center),
                text(body).size(theme::SIZE_BODY).color(p.n.ink),
            ]
            .spacing(theme::S2),
        );
    }
    Some(
        column![
            column![
                widgets::section(p, t("sync-edited-title"), None),
                text(t("sync-edited-body"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_2),
            ]
            .spacing(theme::S2),
            container(list)
                .padding([theme::S5, theme::S5])
                .width(Length::Fill)
                .style(move |_| p.pane(false, false)),
        ]
        .spacing(theme::S4)
        .into(),
    )
}

/// "on Laptop" for the notes of model `id`.
pub fn notes_device(app: &App, id: ModelId) -> Option<String> {
    let m = &app.sync.model;
    (m.id == Some(id))
        .then(|| m.attribution.notes.clone())
        .flatten()
        .map(|d| t_args("sync-on-device", &[("device", Arg::str(d))]))
}

/// "on Laptop" for an attempt of the open model.
pub fn attempt_device(app: &App, attempt: printdex_core::AttemptId) -> Option<String> {
    app.sync
        .model
        .attribution
        .attempts
        .get(&attempt)
        .map(|d| t_args("sync-on-device", &[("device", Arg::str(d.clone()))]))
}
