//! Settings › Backup: the target setup form, per-library model switches,
//! schedule, history, the sidebar status line and the prompt for secrets
//! that aren't stored on this computer.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use iced::Task;
use printdex_core::backup::{
    Attention, BackupConfig, BackupKind, BackupRun, BackupScheduler, BackupScope, ModelSchedule,
    Target, TargetCheck, TargetConfig, TargetSecrets, default_models_on,
};
use printdex_core::{BackupError, CoreError, Library, LibraryId};

use crate::app::{App, Message, Screen, SettingsSection};
use crate::i18n::t;

/// How many past jobs the history shows.
pub const HISTORY: u32 = 50;

/// Upload limits offered for remote targets, in KiB/s (`None`: no limit).
pub const UPLOAD_LIMITS: [Option<u32>; 5] = [
    None,
    Some(1024),
    Some(5 * 1024),
    Some(10 * 1024),
    Some(50 * 1024),
];

/// A kind of backup target, as picked in the setup form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    S3,
    WebDav,
    Sftp,
}

/// The target kinds this system offers: SFTP uses the system's ssh, which
/// Printdex doesn't support on Windows.
pub fn target_kinds(windows: bool) -> Vec<Kind> {
    let mut kinds = vec![Kind::Folder, Kind::S3, Kind::WebDav];
    if !windows {
        kinds.push(Kind::Sftp);
    }
    kinds
}

/// Whether `t` needs a secret besides the passphrase (S3 secret key,
/// WebDAV password).
pub fn needs_secret(t: &Target) -> bool {
    matches!(t, Target::S3 { .. } | Target::WebDav { .. })
}

/// A text field of the setup form or the secrets prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Path,
    Endpoint,
    Bucket,
    Region,
    KeyId,
    Prefix,
    Url,
    User,
    Host,
    Port,
    SftpPath,
    KeyFile,
    Secret,
    Passphrase,
    Passphrase2,
}

/// The result of "Test connection".
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Probe {
    #[default]
    None,
    Testing,
    Ok(TargetCheck),
    Failed(String),
}

/// The setup form for a new target.
#[derive(Clone, Default)]
pub struct Form {
    pub kind: Option<Kind>,
    pub path: String,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub key_id: String,
    pub prefix: String,
    pub url: String,
    pub user: String,
    pub host: String,
    pub port: String,
    pub sftp_path: String,
    pub key_file: String,
    pub secret: String,
    pub encrypted: bool,
    pub passphrase: String,
    pub passphrase2: String,
    /// "I understand a lost passphrase can't be recovered."
    pub ack: bool,
    /// Keep the secrets in the OS credential store.
    pub remember: bool,
    /// Which libraries' model files to back up.
    pub models: Vec<(LibraryId, bool)>,
    pub probe: Probe,
    /// Save was pressed: the test runs first, then the confirm step.
    pub saving: bool,
    /// Asking before the first model upload.
    pub confirming: bool,
}

impl Form {
    /// A fresh form: a folder target, model switches at their defaults
    /// (nothing is uploaded until the user confirms).
    pub fn new(libraries: &[Library]) -> Form {
        let mut f = Form {
            remember: true,
            port: "22".into(),
            models: libraries
                .iter()
                .map(|l| (l.id, default_models_on(l.kind)))
                .collect(),
            ..Form::default()
        };
        f.set_kind(Kind::Folder);
        f
    }

    /// Encryption starts on for remote targets and off for folders.
    pub fn set_kind(&mut self, kind: Kind) {
        self.kind = Some(kind);
        self.encrypted = kind != Kind::Folder;
        self.probe = Probe::None;
    }

    fn field(&mut self, field: Field) -> &mut String {
        match field {
            Field::Path => &mut self.path,
            Field::Endpoint => &mut self.endpoint,
            Field::Bucket => &mut self.bucket,
            Field::Region => &mut self.region,
            Field::KeyId => &mut self.key_id,
            Field::Prefix => &mut self.prefix,
            Field::Url => &mut self.url,
            Field::User => &mut self.user,
            Field::Host => &mut self.host,
            Field::Port => &mut self.port,
            Field::SftpPath => &mut self.sftp_path,
            Field::KeyFile => &mut self.key_file,
            Field::Secret => &mut self.secret,
            Field::Passphrase => &mut self.passphrase,
            Field::Passphrase2 => &mut self.passphrase2,
        }
    }

    pub fn set(&mut self, field: Field, value: String) {
        *self.field(field) = value;
        if !matches!(field, Field::Passphrase | Field::Passphrase2) {
            self.probe = Probe::None;
        }
    }

    /// The target the form describes, when every required field is filled.
    pub fn target(&self) -> Option<Target> {
        let filled = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
        Some(match self.kind? {
            Kind::Folder => Target::Folder {
                path: PathBuf::from(filled(&self.path)?),
            },
            Kind::S3 => Target::S3 {
                endpoint: filled(&self.endpoint)?,
                bucket: filled(&self.bucket)?,
                region: filled(&self.region).unwrap_or_else(|| "auto".into()),
                key_id: filled(&self.key_id)?,
                prefix: self.prefix.trim().trim_matches('/').to_owned(),
            },
            Kind::WebDav => Target::WebDav {
                url: filled(&self.url)?,
                user: filled(&self.user)?,
            },
            Kind::Sftp => Target::Sftp {
                host: filled(&self.host)?,
                port: self.port.trim().parse().ok()?,
                user: filled(&self.user)?,
                path: filled(&self.sftp_path)?,
                key_file: filled(&self.key_file).map(PathBuf::from),
            },
        })
    }

    pub fn secrets(&self) -> TargetSecrets {
        let remote_secret = self.target().is_some_and(|t| needs_secret(&t));
        TargetSecrets {
            secret: remote_secret.then(|| self.secret.clone()),
            passphrase: if self.encrypted {
                self.passphrase.clone()
            } else {
                String::new()
            },
        }
    }

    /// Why the form can't be tested yet (`None`: it can).
    pub fn test_blocker(&self) -> Option<String> {
        let target = self.target()?;
        if needs_secret(&target) && self.secret.is_empty() {
            return Some(t("backup-need-secret"));
        }
        None
    }

    /// Why the form can't be saved yet (`None`: it can).
    pub fn save_blocker(&self) -> Option<String> {
        if self.target().is_none() {
            return Some(t("backup-need-fields"));
        }
        if let Some(b) = self.test_blocker() {
            return Some(b);
        }
        if self.encrypted {
            if self.passphrase.is_empty() {
                return Some(t("backup-need-passphrase"));
            }
            if self.passphrase != self.passphrase2 {
                return Some(t("backup-passphrase-mismatch"));
            }
            if !self.ack {
                return Some(t("backup-need-ack"));
            }
        }
        None
    }

    pub fn can_save(&self) -> bool {
        !self.saving && self.save_blocker().is_none()
    }

    /// Libraries whose models the form would back up.
    pub fn models_on(&self) -> impl Iterator<Item = LibraryId> + '_ {
        self.models.iter().filter(|(_, on)| *on).map(|(id, _)| *id)
    }
}

/// The prompt for secrets this computer doesn't have.
#[derive(Clone, Default)]
pub struct Prompt {
    pub secret: String,
    pub passphrase: String,
    pub remember: bool,
    pub busy: bool,
    pub error: Option<String>,
}

/// A backup job in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Running {
    pub kind: BackupKind,
    pub done: u64,
    pub total: u64,
}

/// The sidebar footer line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLine {
    /// Set up, nothing backed up yet.
    Idle,
    /// A job runs; the percentage when known.
    Running(Option<u8>),
    /// When the last data backup finished (unix ms).
    Last(i64),
    NeedsAttention,
}

/// What the backup section loads from the engine.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub config: BackupConfig,
    pub runs: Vec<BackupRun>,
    pub secrets_known: bool,
    pub persistent: bool,
    pub estimates: HashMap<LibraryId, u64>,
}

#[derive(Default)]
pub struct BackupUi {
    pub config: Option<BackupConfig>,
    pub runs: Vec<BackupRun>,
    /// Repository size on the target, once measured.
    pub size: Option<u64>,
    /// Bytes of each library's files (what a first model backup reads).
    pub estimates: HashMap<LibraryId, u64>,
    pub form: Option<Form>,
    pub running: Option<Running>,
    pub attention: Vec<Attention>,
    /// The library whose model switch waits for "Back up models".
    pub confirm_models: Option<LibraryId>,
    pub confirm_remove: bool,
    /// Whether the target's secrets are known (else the prompt shows).
    pub secrets_known: bool,
    /// Whether "Remember on this computer" survives a restart here.
    pub persistent: bool,
    pub prompt: Prompt,
    pub error: Option<String>,
    pub scheduler: Option<Arc<BackupScheduler>>,
}

impl BackupUi {
    pub fn target(&self) -> Option<&TargetConfig> {
        self.config.as_ref().and_then(|c| c.targets.first())
    }

    /// When the last data backup finished well.
    pub fn last_ok(&self) -> Option<i64> {
        self.runs
            .iter()
            .filter(|r| r.kind == "data" && r.result == "ok")
            .filter_map(|r| r.finished_at)
            .max()
    }

    /// The sidebar line; `None` while backups aren't set up.
    pub fn status_line(&self) -> Option<StatusLine> {
        self.target()?;
        if let Some(r) = self.running {
            let pct = (r.total > 0)
                .then(|| u8::try_from((r.done.min(r.total) * 100) / r.total).unwrap_or(100));
            return Some(StatusLine::Running(pct));
        }
        if !self.secrets_known || !self.attention.is_empty() {
            return Some(StatusLine::NeedsAttention);
        }
        Some(self.last_ok().map_or(StatusLine::Idle, StatusLine::Last))
    }
}

#[derive(Clone, Debug)]
pub enum Bx {
    Loaded(Result<Loaded, Arc<CoreError>>),
    SchedulerStarted(Arc<BackupScheduler>),
    Size(Result<Option<u64>, Arc<CoreError>>),
    /// The sidebar line: opens Settings › Backup.
    Open,
    StartSetup,
    CancelSetup,
    Kind(Kind),
    Input(Field, String),
    Encrypted(bool),
    Ack(bool),
    Remember(bool),
    FormModels(LibraryId, bool),
    PickFolder,
    FolderPicked(Option<PathBuf>),
    PickKeyFile,
    KeyFilePicked(Option<PathBuf>),
    Test,
    Tested(Result<TargetCheck, Arc<CoreError>>),
    Save,
    ConfirmSave,
    BackToForm,
    Saved(Result<(BackupConfig, TargetSecrets, bool), Arc<CoreError>>),
    Models(LibraryId, bool),
    ConfirmModels,
    CancelModels,
    ModelSchedule(ModelSchedule),
    UploadLimit(Option<u32>),
    ConfigSaved(Result<BackupConfig, Arc<CoreError>>),
    BackUpNow,
    Cancel,
    Ran(Result<(), Arc<CoreError>>),
    AskRemove,
    CancelRemove,
    ConfirmRemove,
    PromptInput(Field, String),
    PromptRemember(bool),
    Unlock,
    Unlocked(Result<(), Arc<CoreError>>),
    DismissError,
}

/// What the user can do about a backup error.
pub fn backup_error_text(e: &BackupError) -> String {
    match e {
        BackupError::WrongPassphrase => t("backup-error-passphrase"),
        BackupError::Credentials => t("backup-error-credentials"),
        BackupError::Unreachable(_) => t("backup-error-unreachable"),
        BackupError::Full => t("backup-error-full"),
        BackupError::ReadOnly => t("backup-error-read-only"),
        BackupError::TargetInvalid => t("backup-error-invalid"),
        BackupError::Unsupported => t("backup-error-unsupported"),
        BackupError::NotConfigured => t("backup-error-not-configured"),
        BackupError::SecretsNeeded => t("backup-error-secrets"),
        BackupError::NoRepository => t("backup-error-no-repository"),
        BackupError::SnapshotNotFound => t("backup-error-snapshot"),
        BackupError::CheckFailed(_) => t("backup-error-check"),
        BackupError::Repository(_) => t("error-other"),
    }
}

fn err_text(e: &CoreError) -> String {
    match e {
        CoreError::Backup(b) => backup_error_text(b),
        CoreError::Cancelled => t("backup-cancelled"),
        other => crate::app::error_text(other),
    }
}

impl App {
    /// Loads the backup settings, history and size estimates.
    pub(crate) fn load_backup(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let config = engine.backup_config().await?;
                let runs = engine.backup_runs(HISTORY).await?;
                let secrets_known = config
                    .targets
                    .first()
                    .is_none_or(|t| engine.target_secrets_known(t));
                let mut estimates = HashMap::new();
                for lib in engine.libraries().await? {
                    estimates.insert(lib.id, engine.estimate_models(lib.id).await?);
                }
                Ok(Loaded {
                    config,
                    runs,
                    secrets_known,
                    persistent: engine.secrets_persistent(),
                    estimates,
                })
            },
            |r: Result<_, CoreError>| Message::Backup(Bx::Loaded(r.map_err(Arc::new))),
        )
    }

    fn measure_backup(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        if self.backup.target().is_none() || !self.backup.secrets_known {
            return Task::none();
        }
        Task::perform(
            async move { engine.backup_size().await.map_err(Arc::new) },
            |r| Message::Backup(Bx::Size(r)),
        )
    }

    /// Starts unattended backups and loads the section (once the engine
    /// is open).
    pub(crate) fn start_backups(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        if let Some(dir) = &self.options.config_dir {
            engine.set_backup_files(vec![
                dir.join(crate::settings::FILE_NAME),
                dir.join(crate::apps::FILE_NAME),
            ]);
        }
        // The scheduler spawns tasks, so it starts on the runtime.
        let start = Task::perform(
            async move { Arc::new(engine.start_backup_scheduler()) },
            |s| Message::Backup(Bx::SchedulerStarted(s)),
        );
        Task::batch([start, self.load_backup()])
    }

    fn save_backup_config(&self, config: BackupConfig) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.configure_backup(config).await.map_err(Arc::new) },
            |r| Message::Backup(Bx::ConfigSaved(r)),
        )
    }

    /// Changes the stored config right away and saves it.
    fn change_config(&mut self, f: impl FnOnce(&mut BackupConfig)) -> Task<Message> {
        let Some(config) = self.backup.config.as_mut() else {
            return Task::none();
        };
        f(config);
        let config = config.clone();
        self.save_backup_config(config)
    }

    fn back_up_now(&mut self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        if self.backup.running.is_some() {
            return Task::none();
        }
        self.backup.error = None;
        Task::perform(
            async move {
                engine
                    .backup_now(BackupScope::All)
                    .await
                    .map(|_| ())
                    .map_err(Arc::new)
            },
            |r| Message::Backup(Bx::Ran(r)),
        )
    }

    /// Saves the setup form: the target, the model switches, then the
    /// secrets.
    fn save_backup_form(&mut self) -> Task<Message> {
        let Some(form) = self.backup.form.as_mut() else {
            return Task::none();
        };
        let Some(target) = form.target() else {
            return Task::none();
        };
        form.confirming = false;
        form.saving = true;
        let mut config = self.backup.config.clone().unwrap_or_default();
        config.targets = vec![TargetConfig {
            id: 0,
            target,
            encrypted: form.encrypted,
        }];
        config.libraries.clone_from(&form.models);
        let secrets = form.secrets();
        let remember = form.remember;
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let saved = engine.configure_backup(config).await?;
                let id = saved.targets.first().map_or(0, |t| t.id);
                let remembered = engine.set_target_secrets(id, &secrets, remember);
                Ok((saved, secrets, remembered))
            },
            |r: Result<_, CoreError>| Message::Backup(Bx::Saved(r.map_err(Arc::new))),
        )
    }

    fn test_backup_form(&mut self) -> Task<Message> {
        let Some(form) = self.backup.form.as_mut() else {
            return Task::none();
        };
        let Some(target) = form.target() else {
            return Task::none();
        };
        if form.test_blocker().is_some() {
            return Task::none();
        }
        form.probe = Probe::Testing;
        let secrets = form.secrets();
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                engine
                    .test_target(&target, &secrets)
                    .await
                    .map_err(Arc::new)
            },
            |r| Message::Backup(Bx::Tested(r)),
        )
    }

    pub fn on_backup(&mut self, msg: Bx) -> Task<Message> {
        match msg {
            Bx::Loaded(Ok(l)) => {
                self.backup.config = Some(l.config);
                self.backup.runs = l.runs;
                self.backup.secrets_known = l.secrets_known;
                self.backup.persistent = l.persistent;
                self.backup.estimates = l.estimates;
                if let Some(engine) = &self.engine {
                    self.backup.attention = engine.backup_attention();
                }
                self.measure_backup()
            }
            Bx::Loaded(Err(e)) | Bx::Size(Err(e)) => {
                tracing::warn!(error = %e, "backup status");
                Task::none()
            }
            Bx::SchedulerStarted(s) => {
                self.backup.scheduler = Some(s);
                Task::none()
            }
            Bx::Size(Ok(size)) => {
                self.backup.size = size;
                Task::none()
            }
            Bx::Open => {
                self.screen = Screen::Settings;
                self.settings_section = SettingsSection::Backup;
                self.load_backup()
            }
            Bx::StartSetup => {
                self.backup.form = Some(Form::new(&self.libraries));
                self.backup.error = None;
                Task::none()
            }
            Bx::CancelSetup => {
                self.backup.form = None;
                Task::none()
            }
            Bx::Kind(kind) => {
                if let Some(f) = &mut self.backup.form {
                    f.set_kind(kind);
                }
                Task::none()
            }
            Bx::Input(field, value) => {
                if let Some(f) = &mut self.backup.form {
                    f.set(field, value);
                }
                Task::none()
            }
            Bx::Encrypted(on) => {
                if let Some(f) = &mut self.backup.form {
                    f.encrypted = on;
                    f.probe = Probe::None;
                }
                Task::none()
            }
            Bx::Ack(on) => {
                if let Some(f) = &mut self.backup.form {
                    f.ack = on;
                }
                Task::none()
            }
            Bx::Remember(on) => {
                if let Some(f) = &mut self.backup.form {
                    f.remember = on;
                }
                Task::none()
            }
            Bx::FormModels(lib, on) => {
                if let Some(f) = &mut self.backup.form
                    && let Some(m) = f.models.iter_mut().find(|(id, _)| *id == lib)
                {
                    m.1 = on;
                }
                Task::none()
            }
            Bx::PickFolder => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(t("backup-pick-folder"))
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                |p| Message::Backup(Bx::FolderPicked(p)),
            ),
            Bx::PickKeyFile => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(t("backup-pick-key"))
                        .pick_file()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                |p| Message::Backup(Bx::KeyFilePicked(p)),
            ),
            Bx::FolderPicked(Some(path)) => {
                if let Some(f) = &mut self.backup.form {
                    f.set(Field::Path, path.display().to_string());
                }
                Task::none()
            }
            Bx::KeyFilePicked(Some(path)) => {
                if let Some(f) = &mut self.backup.form {
                    f.set(Field::KeyFile, path.display().to_string());
                }
                Task::none()
            }
            Bx::FolderPicked(None) | Bx::KeyFilePicked(None) => Task::none(),
            Bx::Test => self.test_backup_form(),
            Bx::Tested(result) => {
                let Some(f) = &mut self.backup.form else {
                    return Task::none();
                };
                match result {
                    Ok(check) => {
                        f.probe = Probe::Ok(check);
                        if f.saving {
                            f.saving = false;
                            if f.models_on().next().is_some() {
                                f.confirming = true;
                                Task::none()
                            } else {
                                self.save_backup_form()
                            }
                        } else {
                            Task::none()
                        }
                    }
                    Err(e) => {
                        f.saving = false;
                        f.probe = Probe::Failed(err_text(&e));
                        Task::none()
                    }
                }
            }
            Bx::Save => {
                let Some(f) = &mut self.backup.form else {
                    return Task::none();
                };
                if !f.can_save() {
                    return Task::none();
                }
                f.saving = true;
                self.test_backup_form()
            }
            Bx::ConfirmSave => self.save_backup_form(),
            Bx::BackToForm => {
                if let Some(f) = &mut self.backup.form {
                    f.confirming = false;
                }
                Task::none()
            }
            Bx::Saved(Ok((config, _secrets, remembered))) => {
                let remember = self.backup.form.as_ref().is_some_and(|f| f.remember);
                if remember && !remembered {
                    self.backup.error = Some(t("backup-not-remembered"));
                }
                self.backup.form = None;
                self.backup.config = Some(config);
                self.backup.secrets_known = true;
                Task::batch([self.load_backup(), self.back_up_now()])
            }
            Bx::Saved(Err(e)) => {
                if let Some(f) = &mut self.backup.form {
                    f.saving = false;
                    f.probe = Probe::Failed(err_text(&e));
                }
                Task::none()
            }
            Bx::Models(lib, true) => {
                // Turning a library on uploads its files: ask first.
                self.backup.confirm_models = Some(lib);
                Task::none()
            }
            Bx::Models(lib, false) => {
                self.backup.confirm_models = None;
                self.change_config(|c| set_models(c, lib, false))
            }
            Bx::ConfirmModels => match self.backup.confirm_models.take() {
                Some(lib) => self.change_config(|c| set_models(c, lib, true)),
                None => Task::none(),
            },
            Bx::CancelModels => {
                self.backup.confirm_models = None;
                Task::none()
            }
            Bx::ModelSchedule(s) => self.change_config(|c| c.schedule.models = s),
            Bx::UploadLimit(l) => self.change_config(|c| c.schedule.upload_limit_kib = l),
            Bx::ConfigSaved(Ok(config)) => {
                self.backup.config = Some(config);
                Task::none()
            }
            Bx::ConfigSaved(Err(e)) => {
                self.backup.error = Some(err_text(&e));
                self.load_backup()
            }
            Bx::BackUpNow => self.back_up_now(),
            Bx::Cancel => {
                if let Some(engine) = &self.engine {
                    engine.cancel_backup();
                }
                Task::none()
            }
            Bx::Ran(Ok(())) => Task::none(),
            Bx::Ran(Err(e)) => {
                if !matches!(*e, CoreError::Cancelled) {
                    self.backup.error = Some(err_text(&e));
                }
                self.load_backup()
            }
            Bx::AskRemove => {
                self.backup.confirm_remove = true;
                Task::none()
            }
            Bx::CancelRemove => {
                self.backup.confirm_remove = false;
                Task::none()
            }
            Bx::ConfirmRemove => {
                self.backup.confirm_remove = false;
                self.backup.size = None;
                self.backup.attention.clear();
                self.change_config(|c| c.targets.clear())
            }
            Bx::PromptInput(field, value) => {
                match field {
                    Field::Secret => self.backup.prompt.secret = value,
                    _ => self.backup.prompt.passphrase = value,
                }
                Task::none()
            }
            Bx::PromptRemember(on) => {
                self.backup.prompt.remember = on;
                Task::none()
            }
            Bx::Unlock => {
                let (Some(engine), Some(t)) = (self.engine.clone(), self.backup.target().cloned())
                else {
                    return Task::none();
                };
                let p = &mut self.backup.prompt;
                p.busy = true;
                p.error = None;
                let secrets = TargetSecrets {
                    secret: needs_secret(&t.target).then(|| p.secret.clone()),
                    passphrase: if t.encrypted {
                        p.passphrase.clone()
                    } else {
                        String::new()
                    },
                };
                let remember = p.remember;
                Task::perform(
                    async move {
                        engine.test_target(&t.target, &secrets).await?;
                        engine.set_target_secrets(t.id, &secrets, remember);
                        Ok(())
                    },
                    |r: Result<(), CoreError>| Message::Backup(Bx::Unlocked(r.map_err(Arc::new))),
                )
            }
            Bx::Unlocked(Ok(())) => {
                self.backup.prompt = Prompt::default();
                self.backup.secrets_known = true;
                self.load_backup()
            }
            Bx::Unlocked(Err(e)) => {
                self.backup.prompt.busy = false;
                self.backup.prompt.error = Some(err_text(&e));
                Task::none()
            }
            Bx::DismissError => {
                self.backup.error = None;
                Task::none()
            }
        }
    }

    /// Engine events about backups.
    pub(crate) fn on_backup_event(&mut self, event: printdex_core::EngineEvent) -> Task<Message> {
        use printdex_core::EngineEvent as E;
        match event {
            E::BackupStarted { kind } => {
                self.backup.running = Some(Running {
                    kind,
                    done: 0,
                    total: 0,
                });
                Task::none()
            }
            E::BackupProgress { kind, done, total } => {
                self.backup.running = Some(Running { kind, done, total });
                Task::none()
            }
            E::BackupFinished { .. } => {
                self.backup.running = None;
                self.load_backup()
            }
            E::BackupFailed { error, .. } => {
                self.backup.running = None;
                if matches!(
                    *error,
                    CoreError::Backup(BackupError::SecretsNeeded | BackupError::WrongPassphrase)
                ) {
                    self.backup.secrets_known = false;
                }
                self.load_backup()
            }
            E::BackupNeedsAttention(list) => {
                self.backup.attention = list;
                Task::none()
            }
            _ => Task::none(),
        }
    }
}

/// Sets `lib`'s model switch in `config`.
fn set_models(config: &mut BackupConfig, lib: LibraryId, on: bool) {
    match config.libraries.iter_mut().find(|(id, _)| *id == lib) {
        Some(entry) => entry.1 = on,
        None => config.libraries.push((lib, on)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(crate::app::Options::in_memory(PathBuf::from(
            "/nonexistent",
        )))
    }

    fn configured(a: &mut App) {
        a.backup.config = Some(BackupConfig {
            targets: vec![TargetConfig {
                id: 1,
                target: Target::Folder {
                    path: "/backups".into(),
                },
                encrypted: false,
            }],
            ..BackupConfig::default()
        });
        a.backup.secrets_known = true;
    }

    #[test]
    fn setup_requires_ack_when_encrypted() {
        let mut f = Form::new(&[]);
        f.set(Field::Path, "/media/usb/backups".into());
        // A folder starts unencrypted and can be saved as it is.
        assert!(!f.encrypted);
        assert!(f.can_save());
        f.encrypted = true;
        assert!(!f.can_save());
        f.passphrase = "orbit violin".into();
        f.passphrase2 = "orbit violin".into();
        assert_eq!(f.save_blocker(), Some(t("backup-need-ack")));
        f.ack = true;
        assert!(f.can_save());
        f.passphrase2 = "orbit violín".into();
        assert_eq!(f.save_blocker(), Some(t("backup-passphrase-mismatch")));
        // Remote targets start encrypted and need their secret.
        let mut s = Form::new(&[]);
        s.set_kind(Kind::S3);
        assert!(s.encrypted);
        for (field, v) in [
            (Field::Endpoint, "https://s3.example.com"),
            (Field::Bucket, "prints"),
            (Field::KeyId, "AKIA"),
        ] {
            s.set(field, v.into());
        }
        assert_eq!(s.save_blocker(), Some(t("backup-need-secret")));
        s.secret = "x".into();
        assert_eq!(s.save_blocker(), Some(t("backup-need-passphrase")));
    }

    #[test]
    fn first_model_backup_needs_confirm() {
        let mut a = app();
        configured(&mut a);
        let lib = LibraryId(printdex_core::ids::Uuid::nil());
        let _ = a.on_backup(Bx::Models(lib, true));
        // Nothing is switched on until the user confirms.
        assert_eq!(a.backup.confirm_models, Some(lib));
        assert!(!a.backup.config.as_ref().unwrap().models_on(lib));
        let _ = a.on_backup(Bx::CancelModels);
        assert!(!a.backup.config.as_ref().unwrap().models_on(lib));
        let _ = a.on_backup(Bx::Models(lib, true));
        let _ = a.on_backup(Bx::ConfirmModels);
        assert!(a.backup.config.as_ref().unwrap().models_on(lib));
        assert_eq!(a.backup.confirm_models, None);
        // Switching off needs no confirmation.
        let _ = a.on_backup(Bx::Models(lib, false));
        assert!(!a.backup.config.as_ref().unwrap().models_on(lib));

        // The setup form confirms before saving when models are on.
        let mut f = Form::new(&[]);
        f.models = vec![(lib, true)];
        f.set(Field::Path, "/backups".into());
        f.saving = true;
        a.backup.form = Some(f);
        let _ = a.on_backup(Bx::Tested(Ok(TargetCheck { has_backup: false })));
        let f = a.backup.form.as_ref().unwrap();
        assert!(f.confirming);
        assert!(!f.saving);
    }

    #[test]
    fn status_line_states() {
        let mut a = app();
        assert_eq!(a.backup.status_line(), None);
        configured(&mut a);
        assert_eq!(a.backup.status_line(), Some(StatusLine::Idle));
        let _ = a.on_backup_event(printdex_core::EngineEvent::BackupStarted {
            kind: BackupKind::Data,
        });
        assert_eq!(a.backup.status_line(), Some(StatusLine::Running(None)));
        let _ = a.on_backup_event(printdex_core::EngineEvent::BackupProgress {
            kind: BackupKind::Data,
            done: 42,
            total: 100,
        });
        assert_eq!(a.backup.status_line(), Some(StatusLine::Running(Some(42))));
        let _ = a.on_backup_event(printdex_core::EngineEvent::BackupFinished {
            kind: BackupKind::Data,
            snapshot: None,
        });
        a.backup.runs = vec![BackupRun {
            id: 1,
            kind: "data".into(),
            library_id: None,
            started_at: 1_000,
            finished_at: Some(2_000),
            result: "ok".into(),
            bytes_added: 10,
            error: None,
        }];
        assert_eq!(a.backup.status_line(), Some(StatusLine::Last(2_000)));
        let _ = a.on_backup_event(printdex_core::EngineEvent::BackupNeedsAttention(vec![
            Attention::TargetFull,
        ]));
        assert_eq!(a.backup.status_line(), Some(StatusLine::NeedsAttention));
        a.backup.attention.clear();
        a.backup.secrets_known = false;
        assert_eq!(a.backup.status_line(), Some(StatusLine::NeedsAttention));
    }

    #[test]
    fn sftp_hidden_on_windows() {
        assert!(!target_kinds(true).contains(&Kind::Sftp));
        assert!(target_kinds(false).contains(&Kind::Sftp));
        assert_eq!(target_kinds(true).len(), 3);
    }
}
