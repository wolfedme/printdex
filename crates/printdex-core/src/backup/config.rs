//! Backup settings: targets, schedule and per-library model switches, and
//! the engine commands that read and change them.

use serde::{Deserialize, Serialize};

use super::repo::{Repo, RepoOptions, Target, TargetSecrets, probe};
use crate::engine::Engine;
use crate::error::{BackupError, Result};
use crate::ids::LibraryId;

/// How often model files are backed up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelSchedule {
    #[default]
    Daily,
    Weekly,
    /// Only on "Back up now".
    Manual,
}

/// Settings that are not per target or per library.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Schedule {
    pub models: ModelSchedule,
    /// Upload limit for remote targets, in KiB/s.
    pub upload_limit_kib: Option<u32>,
}

/// A target with its settings (never its secrets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetConfig {
    /// 0 for a target not saved yet.
    pub id: i64,
    pub target: Target,
    /// Whether the repository has a passphrase.
    pub encrypted: bool,
}

/// Everything about backups the user chose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackupConfig {
    pub targets: Vec<TargetConfig>,
    pub schedule: Schedule,
    /// Libraries whose model backup was switched on or off.
    pub libraries: Vec<(LibraryId, bool)>,
}

impl BackupConfig {
    /// Whether `library`'s model files are backed up (off until chosen).
    pub fn models_on(&self, library: LibraryId) -> bool {
        self.libraries.iter().any(|(id, on)| *id == library && *on)
    }
}

/// Whether model backup starts switched on for a library of `kind` in the
/// setup screen: on for local and removable drives, off for network
/// shares (usually backed up by the NAS itself). Nothing is uploaded
/// until the user confirms.
pub fn default_models_on(kind: crate::db::LibraryKind) -> bool {
    !matches!(kind, crate::db::LibraryKind::Network)
}

/// What [`Engine::test_target`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetCheck {
    /// A Printdex (restic) backup is already there, and the passphrase
    /// opens it.
    pub has_backup: bool,
}

const SCHEDULE_KEY: &str = "backup_schedule";

pub(crate) fn kind(t: &Target) -> &'static str {
    match t {
        Target::Folder { .. } => "folder",
        Target::S3 { .. } => "s3",
        Target::WebDav { .. } => "web-dav",
        Target::Sftp { .. } => "sftp",
    }
}

/// Whether the target needs a secret besides the passphrase.
pub(crate) fn needs_secret(t: &Target) -> bool {
    matches!(t, Target::S3 { .. } | Target::WebDav { .. })
}

impl Engine {
    fn secret_key(&self, target: i64, what: &str) -> String {
        format!("{}/target-{target}/{what}", self.install)
    }

    /// The backup settings.
    pub async fn backup_config(&self) -> Result<BackupConfig> {
        let mut targets = Vec::new();
        for row in self.db.backup_targets().await? {
            match serde_json::from_str::<Target>(&row.settings) {
                Ok(target) => targets.push(TargetConfig {
                    id: row.id,
                    target,
                    encrypted: row.encrypted,
                }),
                Err(e) => tracing::warn!(error = %e, id = row.id, "unreadable backup target"),
            }
        }
        Ok(BackupConfig {
            targets,
            schedule: self.db.meta_json(SCHEDULE_KEY).await?.unwrap_or_default(),
            libraries: self.db.backup_libraries().await?,
        })
    }

    /// Saves the backup settings and returns them as stored (new targets
    /// get their ids). Removed targets' secrets are forgotten.
    pub async fn configure_backup(&self, config: BackupConfig) -> Result<BackupConfig> {
        let before = self.db.backup_targets().await?;
        let rows: Vec<_> = config
            .targets
            .iter()
            .map(|t| {
                let settings =
                    serde_json::to_string(&t.target).map_err(|_| BackupError::TargetInvalid)?;
                Ok((t.id, kind(&t.target).to_owned(), settings, t.encrypted))
            })
            .collect::<Result<_>>()?;
        let after = self.db.set_backup_targets(&rows).await?;
        for gone in before.iter().filter(|b| after.iter().all(|a| a.id != b.id)) {
            for what in ["secret", "passphrase"] {
                self.secrets.delete(&self.secret_key(gone.id, what));
            }
        }
        self.db
            .set_meta_json(SCHEDULE_KEY, &config.schedule)
            .await?;
        self.db.set_backup_libraries(&config.libraries).await?;
        self.backup_config().await
    }

    /// Stores a target's secrets for this session and, with `remember`,
    /// in the OS credential store. Returns whether they were remembered
    /// (false when there is no credential store).
    pub fn set_target_secrets(&self, target: i64, secrets: &TargetSecrets, remember: bool) -> bool {
        let mut stored = true;
        if let Some(s) = &secrets.secret {
            stored &= self
                .secrets
                .set(&self.secret_key(target, "secret"), s, remember);
        }
        stored &= self.secrets.set(
            &self.secret_key(target, "passphrase"),
            &secrets.passphrase,
            remember,
        );
        stored
    }

    /// A target's secrets, if all it needs are known.
    pub(crate) fn target_secrets(&self, t: &TargetConfig) -> Option<TargetSecrets> {
        let secret = if needs_secret(&t.target) {
            Some(self.secrets.get(&self.secret_key(t.id, "secret"))?)
        } else {
            None
        };
        let passphrase = if t.encrypted {
            self.secrets.get(&self.secret_key(t.id, "passphrase"))?
        } else {
            String::new()
        };
        Some(TargetSecrets { secret, passphrase })
    }

    /// Whether every secret `t` needs is known (else the app asks).
    pub fn target_secrets_known(&self, t: &TargetConfig) -> bool {
        self.target_secrets(t).is_some()
    }

    /// Whether remembered secrets survive a restart here.
    pub fn secrets_persistent(&self) -> bool {
        self.secrets.persistent()
    }

    /// Uses `store` for backup secrets instead of the OS credential store
    /// (tests, and the CLI's `--no-keyring`).
    #[must_use]
    pub fn with_secret_store(mut self, store: std::sync::Arc<super::secrets::Store>) -> Engine {
        self.secrets = store;
        self
    }

    /// Checks that `target` can be reached and written (writes and removes
    /// a small probe file), and whether a backup there opens with
    /// `secrets`. Creates nothing else.
    pub async fn test_target(
        &self,
        target: &Target,
        secrets: &TargetSecrets,
    ) -> Result<TargetCheck> {
        let (target, secrets) = (target.clone(), secrets.clone());
        tokio::task::spawn_blocking(move || {
            probe(&target, &secrets)?;
            match Repo::open(&target, &secrets, &RepoOptions::default()) {
                Ok(_) => Ok(TargetCheck { has_backup: true }),
                Err(crate::CoreError::Backup(BackupError::NoRepository)) => {
                    Ok(TargetCheck { has_backup: false })
                }
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|e| BackupError::Repository(e.to_string()))?
    }

    /// Changes target `id`'s passphrase ("" turns encryption off). Existing
    /// snapshots stay readable with the new one.
    pub async fn change_passphrase(&self, id: i64, new: &str, remember: bool) -> Result<bool> {
        let config = self.backup_config().await?;
        let t = config
            .targets
            .into_iter()
            .find(|t| t.id == id)
            .ok_or(BackupError::TargetInvalid)?;
        let secrets = self
            .target_secrets(&t)
            .ok_or(BackupError::WrongPassphrase)?;
        let new_owned = new.to_owned();
        let target = t.target.clone();
        tokio::task::spawn_blocking(move || {
            let mut repo = Repo::open(&target, &secrets, &RepoOptions::default())?;
            repo.change_passphrase(&new_owned)
        })
        .await
        .map_err(|e| BackupError::Repository(e.to_string()))??;
        self.db.set_target_encrypted(id, !new.is_empty()).await?;
        Ok(self
            .secrets
            .set(&self.secret_key(id, "passphrase"), new, remember))
    }
}
