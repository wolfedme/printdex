//! Settings › Backup and the sidebar's backup line.

use iced::widget::{
    Space, button, checkbox, column, container, progress_bar, row, text, text_input, toggler,
};
use iced::{Alignment, Element, Length};
use printdex_core::backup::{Attention, ModelSchedule, Target};

use super::settings::{heading, setting, small_button};
use super::widgets::{self, icon_button, segmented};
use crate::app::{App, Message};
use crate::backup_ui::{
    BackupUi, Bx, Field, Form, Kind, Probe, StatusLine, UPLOAD_LIMITS, needs_secret, target_kinds,
};
use crate::format::size;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

const LABEL_WIDTH: f32 = 160.0;
const WHEN_WIDTH: f32 = 150.0;
const RESULT_WIDTH: f32 = 110.0;
const ADDED_WIDTH: f32 = 90.0;

/// "14:02" today, else "3 Oct 2026, 14:02", in the OS time zone.
pub fn when(ms: i64) -> String {
    let Ok(ts) = jiff::Timestamp::from_millisecond(ms) else {
        return String::new();
    };
    let tz = jiff::tz::TimeZone::system();
    let at = ts.to_zoned_datetime(tz);
    let time = at.strftime("%H:%M").to_string();
    if at.date() == jiff::ZonedDateTime::now().date() {
        time
    } else {
        format!("{}, {time}", widgets::date(ms))
    }
}

/// "3 Oct 2026, 14:02" in the OS time zone.
pub fn when_full(ms: i64) -> String {
    let Ok(ts) = jiff::Timestamp::from_millisecond(ms) else {
        return String::new();
    };
    let at = ts.to_zoned_datetime(jiff::tz::TimeZone::system());
    format!("{}, {}", widgets::date(ms), at.strftime("%H:%M"))
}

/// The sidebar footer line, while backups are set up.
pub fn status<'a>(b: &BackupUi, p: Palette) -> Option<Element<'a, Message>> {
    let line = b.status_line()?;
    let (label, color, icon) = match line {
        StatusLine::Idle => (t("backup-status-idle"), p.n.on_rail_2, Icon::Backup),
        StatusLine::Running(Some(pct)) => (
            t_args(
                "backup-status-running",
                &[("percent", Arg::count(u64::from(pct)))],
            ),
            p.n.on_rail,
            Icon::Refresh,
        ),
        StatusLine::Running(None) => (t("backup-status-starting"), p.n.on_rail, Icon::Refresh),
        StatusLine::Last(ms) => (
            t_args("backup-status-last", &[("time", Arg::str(when(ms)))]),
            p.n.on_rail_2,
            Icon::Backup,
        ),
        StatusLine::NeedsAttention => (t("backup-status-attention"), p.status.fail, Icon::Alert),
    };
    Some(
        button(
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
        .on_press(Message::Backup(Bx::Open))
        .into(),
    )
}

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let b = &app.backup;
    let mut col = column![heading(p, t("settings-backup"), t("backup-body"))].spacing(theme::S6);
    if let Some(e) = &b.error {
        col = col.push(error_banner(p, e.clone()));
    }
    if app.restore.wizard.is_some() {
        return col.push(super::restore::wizard(app, p)).into();
    }
    let body: Element<'_, Message> = match (&b.config, &b.form) {
        (_, Some(form)) if form.confirming => confirm_setup(app, p, form),
        (_, Some(form)) => setup(app, p, form),
        (None, None) => text(t("backup-loading"))
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .into(),
        (Some(_), None) if b.target().is_none() => empty(p),
        (Some(_), None) => configured(app, p),
    };
    col.push(body).into()
}

pub(super) fn error_banner<'a>(p: Palette, e: String) -> Element<'a, Message> {
    error_banner_with(p, e, Message::Backup(Bx::DismissError))
}

pub(super) fn error_banner_with<'a>(
    p: Palette,
    e: String,
    dismiss: Message,
) -> Element<'a, Message> {
    container(
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(e)
                .size(theme::SIZE_BODY)
                .color(p.n.ink)
                .width(Length::Fill),
            small_button(p, t("backup-dismiss"), dismiss),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .padding([theme::S4, theme::S5])
    .width(Length::Fill)
    .style(move |_| p.pane(false, false))
    .into()
}

pub(super) fn sub_heading<'a>(p: Palette, title: String, body: String) -> Element<'a, Message> {
    column![
        text(title)
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        text(body).size(theme::SIZE_CAPTION).color(p.n.ink_2),
    ]
    .spacing(theme::S2)
    .into()
}

fn empty<'a>(p: Palette) -> Element<'a, Message> {
    container(
        column![
            icons::icon(Icon::Backup, 28.0, p.n.ink_2),
            text(t("backup-empty-title"))
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_BOLD)
                .color(p.n.ink),
            text(t("backup-empty-body"))
                .size(theme::SIZE_BODY)
                .color(p.n.ink_2),
            Space::new().height(theme::S2),
            row![
                icon_button(
                    p,
                    Icon::Plus,
                    t("backup-set-up"),
                    Palette::button_primary,
                    Some(Message::Backup(Bx::StartSetup)),
                ),
                icon_button(
                    p,
                    Icon::Undo,
                    t("restore-start"),
                    Palette::button_secondary,
                    Some(Message::Restore(crate::restore_ui::Rx::Start)),
                ),
            ]
            .spacing(theme::S3),
        ]
        .spacing(theme::S3),
    )
    .padding(theme::S7)
    .width(Length::Fill)
    .style(move |_| p.pane(false, false))
    .into()
}

pub(super) fn kind_label(k: Kind) -> String {
    match k {
        Kind::Folder => t("backup-kind-folder"),
        Kind::S3 => t("backup-kind-s3"),
        Kind::WebDav => t("backup-kind-webdav"),
        Kind::Sftp => t("backup-kind-sftp"),
    }
}

pub(super) fn input<'a>(
    p: Palette,
    label: String,
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> Message + 'a,
    secure: bool,
) -> Element<'a, Message> {
    row![
        text(label)
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(LABEL_WIDTH),
        text_input(placeholder, value)
            .on_input(on_input)
            .secure(secure)
            .size(theme::SIZE_BODY)
            .padding([7.0, theme::S4])
            .style(move |_, status| p.search_field(status)),
    ]
    .spacing(theme::S4)
    .align_y(Alignment::Center)
    .into()
}

fn field<'a>(
    p: Palette,
    label: String,
    placeholder: &str,
    value: &str,
    f: Field,
) -> Element<'a, Message> {
    let secure = matches!(f, Field::Secret | Field::Passphrase | Field::Passphrase2);
    input(
        p,
        label,
        placeholder,
        value,
        move |v| Message::Backup(Bx::Input(f, v)),
        secure,
    )
}

pub(super) fn check<'a>(
    p: Palette,
    label: String,
    on: bool,
    msg: impl Fn(bool) -> Message + 'a,
) -> Element<'a, Message> {
    checkbox(on)
        .label(label)
        .size(16)
        .text_size(theme::SIZE_BODY)
        .spacing(theme::S3)
        .style(move |_, status| p.checkbox(status))
        .on_toggle(msg)
        .into()
}

fn library_name(app: &App, id: printdex_core::LibraryId) -> String {
    app.library(id)
        .map(|l| l.display_name.clone())
        .unwrap_or_else(|| t("backup-removed-library"))
}

fn estimate(app: &App, id: printdex_core::LibraryId) -> String {
    match app.backup.estimates.get(&id) {
        Some(bytes) => t_args("backup-estimate", &[("size", Arg::str(size(*bytes)))]),
        None => String::new(),
    }
}

fn setup<'a>(app: &'a App, p: Palette, f: &'a Form) -> Element<'a, Message> {
    let kind = f.kind.unwrap_or(Kind::Folder);
    let kinds: Vec<_> = target_kinds(cfg!(windows))
        .into_iter()
        .map(|k| (k, None, Some(kind_label(k))))
        .collect();
    let mut place = column![
        sub_heading(p, t("backup-where"), t("backup-where-body")),
        segmented(p, &kinds, kind, |k| Message::Backup(Bx::Kind(k))),
    ]
    .spacing(theme::S4);
    place = place.push(target_fields(
        p,
        f,
        |f, v| Message::Backup(Bx::Input(f, v)),
        Message::Backup(Bx::PickFolder),
        Message::Backup(Bx::PickKeyFile),
        "/media/backup/printdex",
    ));
    let (probe_text, probe_color) = match &f.probe {
        Probe::None => (String::new(), p.n.ink_2),
        Probe::Testing => (t("backup-testing"), p.n.ink_2),
        Probe::Ok(c) if c.has_backup => (t("backup-test-existing"), p.status.ok),
        Probe::Ok(_) => (t("backup-test-ok"), p.status.ok),
        Probe::Failed(e) => (e.clone(), p.status.fail),
    };
    let can_test = f.target().is_some() && f.test_blocker().is_none() && f.probe != Probe::Testing;
    place = place.push(
        row![
            icon_button(
                p,
                Icon::Link,
                t("backup-test"),
                Palette::button_secondary,
                can_test.then_some(Message::Backup(Bx::Test)),
            ),
            text(probe_text)
                .size(theme::SIZE_CAPTION)
                .color(probe_color)
                .width(Length::Fill),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    );

    // Encryption.
    let encrypt = toggler(f.encrypted)
        .size(20)
        .style(move |_, status| p.toggler(status))
        .on_toggle(|on| Message::Backup(Bx::Encrypted(on)));
    let mut lock = column![setting(
        p,
        t("backup-encrypt"),
        if kind == Kind::Folder {
            t("backup-encrypt-body-folder")
        } else {
            t("backup-encrypt-body-remote")
        },
        encrypt.into()
    )]
    .spacing(theme::S4);
    if f.encrypted {
        lock = lock
            .push(field(
                p,
                t("backup-field-passphrase"),
                "",
                &f.passphrase,
                Field::Passphrase,
            ))
            .push(field(
                p,
                t("backup-field-passphrase2"),
                "",
                &f.passphrase2,
                Field::Passphrase2,
            ))
            .push(check(p, t("backup-ack"), f.ack, |on| {
                Message::Backup(Bx::Ack(on))
            }));
    }
    let has_secrets = f.encrypted || f.target().is_some_and(|t| needs_secret(&t));
    if has_secrets {
        lock = lock.push(check(p, t("backup-remember"), f.remember, |on| {
            Message::Backup(Bx::Remember(on))
        }));
        if !app.backup.persistent {
            lock = lock.push(
                text(t("backup-no-keyring"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_3),
            );
        }
    }

    // Model files.
    let mut models = column![
        sub_heading(p, t("backup-models"), t("backup-models-body")),
        widgets::hairline(p),
    ]
    .spacing(theme::S5);
    for (id, on) in &f.models {
        let id = *id;
        models = models.push(setting(
            p,
            library_name(app, id),
            estimate(app, id),
            toggler(*on)
                .size(20)
                .style(move |_, status| p.toggler(status))
                .on_toggle(move |on| Message::Backup(Bx::FormModels(id, on)))
                .into(),
        ));
    }
    if f.models.is_empty() {
        models = models.push(
            text(t("backup-models-none"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }

    let blocker = f.save_blocker().unwrap_or_default();
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
            Some(Message::Backup(Bx::CancelSetup)),
        ),
        icon_button(
            p,
            Icon::Check,
            if f.saving {
                t("backup-saving")
            } else {
                t("backup-save")
            },
            Palette::button_primary,
            f.can_save().then_some(Message::Backup(Bx::Save)),
        ),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);

    container(column![place, widgets::hairline(p), lock, models, actions].spacing(theme::S7))
        .padding(theme::S7)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

/// The fields of a folder, S3, WebDAV or SFTP place (backup targets and
/// sync spaces share them).
pub(super) fn target_fields<'a>(
    p: Palette,
    f: &'a Form,
    input: fn(Field, String) -> Message,
    pick_folder: Message,
    pick_key: Message,
    folder_hint: &'a str,
) -> Element<'a, Message> {
    let kind = f.kind.unwrap_or(Kind::Folder);
    let place = column![].spacing(theme::S4);
    let field = |label: String, placeholder: &str, value: &'a str, fld: Field| {
        field_with(p, label, placeholder, value, fld, input)
    };
    match kind {
        Kind::Folder => place.push(
            row![
                container(field(
                    t("backup-field-folder"),
                    folder_hint,
                    &f.path,
                    Field::Path
                ))
                .width(Length::Fill),
                small_button(p, t("backup-choose"), pick_folder),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center),
        ),
        Kind::S3 => place
            .push(field(
                t("backup-field-endpoint"),
                "https://s3.eu-central-1.amazonaws.com",
                &f.endpoint,
                Field::Endpoint,
            ))
            .push(field(
                t("backup-field-bucket"),
                "",
                &f.bucket,
                Field::Bucket,
            ))
            .push(field(
                t("backup-field-region"),
                "auto",
                &f.region,
                Field::Region,
            ))
            .push(field(
                t("backup-field-prefix"),
                "printdex",
                &f.prefix,
                Field::Prefix,
            ))
            .push(field(t("backup-field-key-id"), "", &f.key_id, Field::KeyId))
            .push(field(
                t("backup-field-secret-key"),
                "",
                &f.secret,
                Field::Secret,
            )),
        Kind::WebDav => place
            .push(field(
                t("backup-field-url"),
                "https://cloud.example.com/remote.php/dav/files/me/printdex",
                &f.url,
                Field::Url,
            ))
            .push(field(t("backup-field-user"), "", &f.user, Field::User))
            .push(field(
                t("backup-field-password"),
                "",
                &f.secret,
                Field::Secret,
            )),
        Kind::Sftp => place
            .push(field(
                t("backup-field-host"),
                "nas.local",
                &f.host,
                Field::Host,
            ))
            .push(field(t("backup-field-port"), "22", &f.port, Field::Port))
            .push(field(t("backup-field-user"), "", &f.user, Field::User))
            .push(field(
                t("backup-field-remote-folder"),
                "/volume1/backups/printdex",
                &f.sftp_path,
                Field::SftpPath,
            ))
            .push(
                row![
                    container(field(
                        t("backup-field-key-file"),
                        &t("backup-key-agent"),
                        &f.key_file,
                        Field::KeyFile
                    ))
                    .width(Length::Fill),
                    small_button(p, t("backup-choose"), pick_key),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            ),
    }
    .into()
}

fn field_with<'a>(
    p: Palette,
    label: String,
    placeholder: &str,
    value: &str,
    f: Field,
    input_msg: fn(Field, String) -> Message,
) -> Element<'a, Message> {
    let secure = matches!(f, Field::Secret | Field::Passphrase | Field::Passphrase2);
    input(
        p,
        label,
        placeholder,
        value,
        move |v| input_msg(f, v),
        secure,
    )
}

fn target_summary(t_: &Target) -> (String, String) {
    match t_ {
        Target::Folder { path } => (t("backup-kind-folder"), path.display().to_string()),
        Target::S3 {
            endpoint,
            bucket,
            prefix,
            ..
        } => (
            t("backup-kind-s3"),
            if prefix.is_empty() {
                format!("{bucket} · {endpoint}")
            } else {
                format!("{bucket}/{prefix} · {endpoint}")
            },
        ),
        Target::WebDav { url, user } => (t("backup-kind-webdav"), format!("{user} · {url}")),
        Target::Sftp {
            host,
            port,
            user,
            path,
            ..
        } => (t("backup-kind-sftp"), format!("{user}@{host}:{port}{path}")),
    }
}

fn confirm_setup<'a>(app: &'a App, p: Palette, f: &'a Form) -> Element<'a, Message> {
    let libs: Vec<_> = f.models_on().collect();
    let bytes: u64 = libs
        .iter()
        .filter_map(|id| app.backup.estimates.get(id))
        .sum();
    let place = f
        .target()
        .map(|t_| target_summary(&t_).1)
        .unwrap_or_default();
    container(
        column![
            sub_heading(
                p,
                t("backup-confirm-title"),
                t_args(
                    "backup-confirm-body",
                    &[
                        ("size", Arg::str(size(bytes))),
                        ("count", Arg::count(libs.len() as u64)),
                        ("target", Arg::str(place)),
                    ],
                ),
            ),
            row![
                Space::new().width(Length::Fill),
                icon_button(
                    p,
                    Icon::Undo,
                    t("backup-back"),
                    Palette::button_secondary,
                    Some(Message::Backup(Bx::BackToForm)),
                ),
                icon_button(
                    p,
                    Icon::Backup,
                    t("backup-confirm-yes"),
                    Palette::button_primary,
                    Some(Message::Backup(Bx::ConfirmSave)),
                ),
            ]
            .spacing(theme::S3),
        ]
        .spacing(theme::S6),
    )
    .padding(theme::S7)
    .width(Length::Fill)
    .style(move |_| p.pane(false, false))
    .into()
}

fn attention_text(app: &App, a: &Attention) -> String {
    match a {
        Attention::DataStale => t("backup-attn-data-stale"),
        Attention::ModelsStale(id) => t_args(
            "backup-attn-models-stale",
            &[("library", Arg::str(library_name(app, *id)))],
        ),
        Attention::ConnectLibrary(id) => t_args(
            "backup-attn-connect",
            &[("library", Arg::str(library_name(app, *id)))],
        ),
        Attention::TargetUnreachable => t("backup-attn-unreachable"),
        Attention::TargetFull => t("backup-attn-full"),
        Attention::Credentials => t("backup-attn-credentials"),
        Attention::PassphraseNeeded => t("backup-attn-passphrase"),
        Attention::CheckFailed => t("backup-attn-check"),
    }
}

/// Asks for the secrets this computer doesn't have.
fn prompt<'a>(app: &'a App, p: Palette) -> Option<Element<'a, Message>> {
    let b = &app.backup;
    let target = b.target()?;
    if b.secrets_known {
        return None;
    }
    let pr = &b.prompt;
    let mut col = column![sub_heading(
        p,
        t("backup-prompt-title"),
        t("backup-prompt-body")
    )]
    .spacing(theme::S4);
    if needs_secret(&target.target) {
        col = col.push(input(
            p,
            t("backup-field-secret"),
            "",
            &pr.secret,
            |v| Message::Backup(Bx::PromptInput(Field::Secret, v)),
            true,
        ));
    }
    if target.encrypted {
        col = col.push(input(
            p,
            t("backup-field-passphrase"),
            "",
            &pr.passphrase,
            |v| Message::Backup(Bx::PromptInput(Field::Passphrase, v)),
            true,
        ));
    }
    col = col.push(check(p, t("backup-remember"), pr.remember, |on| {
        Message::Backup(Bx::PromptRemember(on))
    }));
    let mut actions = row![].spacing(theme::S4).align_y(Alignment::Center);
    actions = actions.push(
        text(pr.error.clone().unwrap_or_default())
            .size(theme::SIZE_CAPTION)
            .color(p.status.fail)
            .width(Length::Fill),
    );
    actions = actions.push(icon_button(
        p,
        Icon::Check,
        t("backup-unlock"),
        Palette::button_primary,
        (!pr.busy).then_some(Message::Backup(Bx::Unlock)),
    ));
    col = col.push(actions);
    Some(
        container(col)
            .padding(theme::S6)
            .width(Length::Fill)
            .style(move |_| p.pane(true, false))
            .into(),
    )
}

fn run_kind(app: &App, r: &printdex_core::backup::BackupRun) -> String {
    match r.kind.as_str() {
        "data" => t("backup-run-data"),
        "models" => t_args(
            "backup-run-models",
            &[(
                "library",
                Arg::str(
                    r.library_id
                        .map(|id| library_name(app, id))
                        .unwrap_or_default(),
                ),
            )],
        ),
        "prune" => t("backup-run-prune"),
        _ => t("backup-run-check"),
    }
}

fn configured(app: &App, p: Palette) -> Element<'_, Message> {
    let b = &app.backup;
    let Some(target) = b.target() else {
        return Space::new().into();
    };
    let config = b.config.as_ref();
    let mut col = column![].spacing(theme::S7);
    if let Some(pr) = prompt(app, p) {
        col = col.push(pr);
    }
    // The prompt above already asks for the passphrase.
    let attention: Vec<_> = b
        .attention
        .iter()
        .filter(|a| b.secrets_known || !matches!(a, Attention::PassphraseNeeded))
        .collect();
    if !attention.is_empty() {
        let mut list = column![].spacing(theme::S3);
        for a in attention {
            list = list.push(
                row![
                    icons::icon(Icon::Alert, theme::ICON, p.status.fail),
                    text(attention_text(app, a))
                        .size(theme::SIZE_BODY)
                        .color(p.n.ink),
                ]
                .spacing(theme::S4)
                .align_y(Alignment::Center),
            );
        }
        col = col.push(
            container(list)
                .padding([theme::S5, theme::S6])
                .width(Length::Fill)
                .style(move |_| p.pane(false, false)),
        );
    }

    // The target card.
    let (kind, place) = target_summary(&target.target);
    let lock = if target.encrypted {
        t("backup-encrypted")
    } else {
        t("backup-not-encrypted")
    };
    let used = match b.size {
        Some(bytes) => t_args("backup-size", &[("size", Arg::str(size(bytes)))]),
        None => String::new(),
    };
    let state: Element<'_, Message> = match b.running {
        Some(r) => {
            // Progress is a ratio for display only.
            #[allow(clippy::cast_precision_loss)]
            let ratio = if r.total == 0 {
                0.0
            } else {
                r.done as f32 / r.total as f32
            };
            let what = match r.kind {
                printdex_core::backup::BackupKind::Data => t("backup-run-data"),
                printdex_core::backup::BackupKind::Models(id) => t_args(
                    "backup-run-models",
                    &[("library", Arg::str(library_name(app, id)))],
                ),
                printdex_core::backup::BackupKind::Prune => t("backup-run-prune"),
                printdex_core::backup::BackupKind::Check => t("backup-run-check"),
            };
            column![
                row![
                    text(t_args("backup-running", &[("what", Arg::str(what))]))
                        .size(theme::SIZE_BODY)
                        .color(p.n.ink)
                        .width(Length::Fill),
                    text(if r.total > 0 {
                        format!("{} / {}", size(r.done), size(r.total))
                    } else {
                        String::new()
                    })
                    .size(theme::SIZE_DATA)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_2),
                    small_button(p, t("backup-cancel"), Message::Backup(Bx::Cancel)),
                ]
                .spacing(theme::S4)
                .align_y(Alignment::Center),
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
            ]
            .spacing(theme::S3)
            .into()
        }
        None => row![
            text(match b.last_ok() {
                Some(ms) => t_args("backup-last", &[("time", Arg::str(when(ms)))]),
                None => t("backup-never"),
            })
            .size(theme::SIZE_BODY)
            .color(p.n.ink_2)
            .width(Length::Fill),
            icon_button(
                p,
                Icon::Backup,
                t("backup-now"),
                Palette::button_primary,
                b.secrets_known.then_some(Message::Backup(Bx::BackUpNow)),
            ),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center)
        .into(),
    };
    let mut card = column![
        row![
            icons::icon(
                if target.target.is_folder() {
                    Icon::Folder
                } else {
                    Icon::Link
                },
                theme::ICON,
                p.n.ink_2
            ),
            column![
                text(kind)
                    .size(theme::SIZE_BODY)
                    .font(theme::FONT_STRONG)
                    .color(p.n.ink),
                text(place)
                    .size(theme::SIZE_DATA)
                    .font(theme::FONT_DATA)
                    .color(p.n.ink_2)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(theme::S1)
            .width(Length::Fill),
            column![
                text(lock).size(theme::SIZE_CAPTION).color(p.n.ink_2),
                text(used).size(theme::SIZE_CAPTION).color(p.n.ink_3),
            ]
            .spacing(theme::S1)
            .align_x(Alignment::End),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
        widgets::hairline(p),
        state,
    ]
    .spacing(theme::S5);
    if b.confirm_remove {
        card = card.push(
            row![
                icons::icon(Icon::Alert, theme::ICON, p.status.fail),
                text(t("backup-remove-confirm"))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink)
                    .width(Length::Fill),
                small_button(p, t("settings-cancel"), Message::Backup(Bx::CancelRemove)),
                button(text(t("backup-remove-yes")).size(theme::SIZE_CAPTION))
                    .padding([4.0, theme::S4])
                    .style(move |_, status| p.button_danger(status))
                    .on_press(Message::Backup(Bx::ConfirmRemove)),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center),
        );
    }
    col = col.push(
        container(card)
            .padding(theme::S6)
            .width(Length::Fill)
            .style(move |_| p.pane(false, false)),
    );

    // Model files per library.
    let mut models = column![
        sub_heading(p, t("backup-models"), t("backup-models-body")),
        widgets::hairline(p),
    ]
    .spacing(theme::S5);
    for lib in &app.libraries {
        let id = lib.id;
        let on = config.is_some_and(|c| c.models_on(id));
        models = models.push(setting(
            p,
            lib.display_name.clone(),
            estimate(app, id),
            toggler(on)
                .size(20)
                .style(move |_, status| p.toggler(status))
                .on_toggle(move |on| Message::Backup(Bx::Models(id, on)))
                .into(),
        ));
        if b.confirm_models == Some(id) {
            models = models.push(
                row![
                    icons::icon(Icon::Info, theme::ICON, p.n.ink_2),
                    text(t_args(
                        "backup-models-confirm",
                        &[(
                            "size",
                            Arg::str(size(b.estimates.get(&id).copied().unwrap_or(0)))
                        )],
                    ))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink)
                    .width(Length::Fill),
                    small_button(p, t("settings-cancel"), Message::Backup(Bx::CancelModels)),
                    button(text(t("backup-models-yes")).size(theme::SIZE_CAPTION))
                        .padding([4.0, theme::S4])
                        .style(move |_, status| p.button_primary(status))
                        .on_press(Message::Backup(Bx::ConfirmModels)),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            );
        }
    }
    col = col.push(models);

    // Schedule.
    let schedule = config.map(|c| c.schedule.clone()).unwrap_or_default();
    let mut sched = column![
        sub_heading(p, t("backup-schedule"), t("backup-schedule-body")),
        widgets::hairline(p),
        setting(
            p,
            t("backup-schedule-models"),
            t("backup-schedule-models-body"),
            segmented(
                p,
                &[
                    (ModelSchedule::Daily, None, Some(t("backup-daily"))),
                    (ModelSchedule::Weekly, None, Some(t("backup-weekly"))),
                    (ModelSchedule::Manual, None, Some(t("backup-manual"))),
                ],
                schedule.models,
                |s| Message::Backup(Bx::ModelSchedule(s)),
            ),
        ),
    ]
    .spacing(theme::S5);
    if !target.target.is_folder() {
        let limits: Vec<_> = UPLOAD_LIMITS
            .iter()
            .map(|l| {
                (
                    *l,
                    None,
                    Some(match l {
                        None => t("backup-limit-off"),
                        Some(k) => format!("{} MB/s", k / 1024),
                    }),
                )
            })
            .collect();
        sched = sched.push(setting(
            p,
            t("backup-limit"),
            t("backup-limit-body"),
            segmented(p, &limits, schedule.upload_limit_kib, |l| {
                Message::Backup(Bx::UploadLimit(l))
            }),
        ));
    }
    col = col.push(sched);

    // History.
    let mut history = column![
        sub_heading(p, t("backup-history"), t("backup-history-body")),
        widgets::hairline(p),
    ]
    .spacing(theme::S3);
    if b.runs.is_empty() {
        history = history.push(
            text(t("backup-history-empty"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    for r in &b.runs {
        let (result, color) = match r.result.as_str() {
            "ok" => (t("backup-result-ok"), p.status.ok),
            "running" => (t("backup-result-running"), p.n.ink_2),
            "cancelled" => (t("backup-result-cancelled"), p.n.ink_2),
            "skipped" => (t("backup-result-skipped"), p.status.offline),
            _ => (t("backup-result-failed"), p.status.fail),
        };
        let mut line = row![
            text(when(r.started_at))
                .size(theme::SIZE_DATA)
                .font(theme::FONT_DATA)
                .color(p.n.ink_2)
                .width(WHEN_WIDTH),
            text(run_kind(app, r))
                .size(theme::SIZE_BODY)
                .color(p.n.ink)
                .width(Length::Fill),
            text(result)
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_STRONG)
                .color(color)
                .width(RESULT_WIDTH),
            text(if r.bytes_added > 0 {
                format!("+{}", size(u64::try_from(r.bytes_added).unwrap_or(0)))
            } else {
                String::new()
            })
            .size(theme::SIZE_DATA)
            .font(theme::FONT_DATA)
            .color(p.n.ink_2)
            .width(ADDED_WIDTH)
            .align_x(Alignment::End),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center);
        if r.result == "failed"
            && let Some(e) = &r.error
        {
            line = line.push(iced::widget::tooltip(
                icons::icon(Icon::Info, theme::ICON_DENSE, p.n.ink_3),
                container(text(e.clone()).size(theme::SIZE_CAPTION))
                    .max_width(360)
                    .padding([3.0, theme::S3])
                    .style(move |_| p.tooltip()),
                iced::widget::tooltip::Position::Left,
            ));
        }
        history = history.push(line);
    }
    col = col.push(history);

    col = col.push(
        row![
            Space::new().width(Length::Fill),
            icon_button(
                p,
                Icon::Remove,
                t("backup-remove"),
                Palette::button_secondary,
                (!b.confirm_remove && b.running.is_none())
                    .then_some(Message::Backup(Bx::AskRemove)),
            ),
        ]
        .align_y(Alignment::Center),
    );
    col.into()
}
