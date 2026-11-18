//! Updates: asks once whether to check automatically, checks GitHub
//! releases (on start, every 24 hours, or on "Check now"), downloads in the
//! background and installs only after the user picks "Restart to update".
//!
//! The update feed (`releases.<channel>.json`, which holds every package's
//! SHA-256) must carry a minisign signature from the release key compiled
//! in (`packaging/minisign.pub`). Velopack then checks each downloaded
//! package against that signed feed, so an unsigned or tampered release is
//! never installed. This is the only network call the app makes, and only
//! after consent.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

use iced::Task;
use minisign_verify::{PublicKey, Signature};
use velopack::bundle::Manifest;
use velopack::sources::{HttpSource, UpdateSource};
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, VelopackAsset, VelopackAssetFeed};

use crate::app::{App, Message};

/// Where published releases live (drafts and pre-releases are skipped).
pub const FEED_URL: &str = "https://github.com/wolfedme/printdex/releases/latest/download";

/// The release signing key (see packaging/README.md).
const RELEASE_KEY: &str = include_str!("../../../packaging/minisign.pub");

/// Time between automatic checks.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The release key, or `None` while the file is still the placeholder (a
/// build that can't verify anything never updates).
pub fn release_key() -> Option<PublicKey> {
    PublicKey::decode(RELEASE_KEY).ok()
}

/// Checks `sig` (a `.minisig` file's text) over `data` with `key`.
pub fn verify(key: &PublicKey, data: &[u8], sig: &str) -> Result<(), String> {
    let sig = Signature::decode(sig).map_err(|e| e.to_string())?;
    key.verify(data, &sig, false).map_err(|e| e.to_string())
}

/// Whether an automatic check is due: only with consent, and only when the
/// last one is at least [`CHECK_EVERY`] old.
pub fn check_due(consent: Option<bool>, last: Option<SystemTime>, now: SystemTime) -> bool {
    consent == Some(true)
        && last.is_none_or(|l| now.duration_since(l).unwrap_or_default() >= CHECK_EVERY)
}

/// An [`HttpSource`] whose feed must be signed by the release key.
struct SignedSource {
    base: String,
    key: PublicKey,
    http: HttpSource,
}

impl UpdateSource for SignedSource {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, velopack::Error> {
        let url = format!("{}/releases.{channel}.json", self.base);
        let json = velopack::download::download_url_as_string(&url, None)?;
        let sig = velopack::download::download_url_as_string(&format!("{url}.minisig"), None)?;
        verify(&self.key, json.as_bytes(), &sig)
            .map_err(|e| velopack::Error::InvalidPackage(format!("update feed signature: {e}")))?;
        Ok(serde_json::from_str(&json)?)
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress: Option<Sender<i16>>,
    ) -> Result<(), velopack::Error> {
        self.http
            .download_release_entry(asset, local_file, progress)
    }
}

/// Where the updater stands.
#[derive(Clone, Debug, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded and verified; installs when the user restarts.
    Ready(Box<UpdateInfo>),
    Failed(String),
}

#[derive(Default)]
pub struct Updates {
    /// `None` when this build can't update (not installed, or no key).
    manager: Option<UpdateManager>,
    pub state: UpdateState,
    /// Finished checks only (a failed one counts, so a broken network
    /// isn't retried in a loop).
    pub last_check: Option<SystemTime>,
    /// Tests stand in for an installed build.
    #[cfg(test)]
    pub fake_available: bool,
}

impl Updates {
    /// Set up for the installed app; without a release key or outside a
    /// Velopack install, updates stay off.
    pub fn start() -> Updates {
        let manager = release_key().and_then(|key| {
            let source = SignedSource {
                base: FEED_URL.to_owned(),
                key,
                http: HttpSource::new(FEED_URL),
            };
            UpdateManager::new(source, None, None)
                .inspect_err(|e| tracing::info!(error = %e, "updates off"))
                .ok()
        });
        Updates {
            manager,
            ..Updates::default()
        }
    }

    pub fn available(&self) -> bool {
        #[cfg(test)]
        if self.fake_available {
            return true;
        }
        self.manager.is_some()
    }

    fn busy(&self) -> bool {
        matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Ready(_)
        )
    }
}

#[derive(Clone, Debug)]
pub enum Ux {
    /// The answer to "Check for updates automatically?".
    Consent(bool),
    /// Settings › About › Check now.
    CheckNow,
    /// Hourly: check if one is due.
    Tick,
    Checked(Result<Option<Box<UpdateInfo>>, String>),
    Downloaded(Result<Box<UpdateInfo>, String>),
    /// The only way an update gets installed.
    Restart,
}

impl App {
    pub fn on_updates(&mut self, msg: Ux) -> Task<Message> {
        match msg {
            Ux::Consent(yes) => {
                self.settings.update_checks = Some(yes);
                let save = self.save_settings();
                if yes {
                    Task::batch([save, self.check_updates()])
                } else {
                    save
                }
            }
            Ux::CheckNow => self.check_updates(),
            Ux::Tick => {
                if check_due(
                    self.settings.update_checks,
                    self.updates.last_check,
                    SystemTime::now(),
                ) {
                    self.check_updates()
                } else {
                    Task::none()
                }
            }
            Ux::Checked(result) => {
                self.updates.last_check = Some(SystemTime::now());
                match result {
                    Ok(None) => {
                        self.updates.state = UpdateState::UpToDate;
                        Task::none()
                    }
                    Ok(Some(info)) => self.download_update(*info),
                    Err(e) => {
                        tracing::warn!(error = %e, "update check failed");
                        self.updates.state = UpdateState::Failed(e);
                        Task::none()
                    }
                }
            }
            Ux::Downloaded(result) => {
                self.updates.state = match result {
                    Ok(info) => UpdateState::Ready(info),
                    Err(e) => {
                        tracing::warn!(error = %e, "update download failed");
                        UpdateState::Failed(e)
                    }
                };
                Task::none()
            }
            Ux::Restart => {
                if let (UpdateState::Ready(info), Some(m)) =
                    (&self.updates.state, &self.updates.manager)
                {
                    // Exits the app; Velopack swaps the files and restarts it.
                    if let Err(e) = m.apply_updates_and_restart(&info.TargetFullRelease) {
                        self.updates.state = UpdateState::Failed(e.to_string());
                    }
                }
                Task::none()
            }
        }
    }

    fn check_updates(&mut self) -> Task<Message> {
        let Some(m) = self.updates.manager.clone() else {
            return Task::none();
        };
        if self.updates.busy() {
            return Task::none();
        }
        self.updates.state = UpdateState::Checking;
        Task::perform(
            blocking(move || match m.check_for_updates() {
                Ok(UpdateCheck::UpdateAvailable(info)) => Ok(Some(info)),
                Ok(_) => Ok(None),
                Err(e) => Err(e.to_string()),
            }),
            |r| Message::Updates(Ux::Checked(r)),
        )
    }

    fn download_update(&mut self, info: UpdateInfo) -> Task<Message> {
        let Some(m) = self.updates.manager.clone() else {
            return Task::none();
        };
        self.updates.state = UpdateState::Downloading(info.TargetFullRelease.Version.clone());
        Task::perform(
            blocking(move || {
                m.download_updates(&info, None)
                    .map(|()| Box::new(info))
                    .map_err(|e| e.to_string())
            }),
            |r| Message::Updates(Ux::Downloaded(r)),
        )
    }

    /// The first-launch question, until it is answered (installed builds
    /// only).
    pub fn ask_update_consent(&self) -> bool {
        self.updates.available() && self.settings.update_checks.is_none()
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = include_str!("updates_fixtures/releases.linux.json");
    const FEED_SIG: &str = include_str!("updates_fixtures/releases.linux.json.minisig");
    const TEST_KEY: &str = include_str!("updates_fixtures/test.pub");
    const OTHER_KEY: &str = include_str!("updates_fixtures/other.pub");

    #[test]
    fn no_check_without_consent() {
        let now = SystemTime::now();
        assert!(!check_due(None, None, now));
        assert!(!check_due(Some(false), None, now));
        assert!(check_due(Some(true), None, now));
        let hour_ago = now - Duration::from_secs(3600);
        assert!(!check_due(Some(true), Some(hour_ago), now));
        assert!(check_due(Some(true), Some(now - CHECK_EVERY), now));
    }

    #[test]
    fn signed_feed_verifies_and_bad_signature_is_rejected() {
        let key = PublicKey::decode(TEST_KEY).unwrap();
        assert!(verify(&key, FEED.as_bytes(), FEED_SIG).is_ok());
        // Tampered feed.
        let tampered = FEED.replace("0.2.0", "6.6.6");
        assert!(verify(&key, tampered.as_bytes(), FEED_SIG).is_err());
        // Signed by someone else.
        let other = PublicKey::decode(OTHER_KEY).unwrap();
        assert!(verify(&other, FEED.as_bytes(), FEED_SIG).is_err());
        // Garbage signature.
        assert!(verify(&key, FEED.as_bytes(), "not a signature").is_err());
    }

    fn app() -> App {
        let mut a = App::new(crate::app::Options::in_memory(std::path::PathBuf::from(
            "/nonexistent",
        )));
        a.updates.fake_available = true;
        a
    }

    #[test]
    fn consent_banner_once() {
        let mut a = app();
        assert!(a.ask_update_consent());
        let _ = a.on_updates(Ux::Consent(false));
        assert_eq!(a.settings.update_checks, Some(false));
        assert!(!a.ask_update_consent());
        // The answer survives a restart.
        let saved = crate::settings::Settings::parse(&a.settings.to_toml());
        assert_eq!(saved.update_checks, Some(false));
        // A build that can't update never asks.
        let mut b = app();
        b.updates.fake_available = false;
        assert!(!b.ask_update_consent());
    }

    #[test]
    fn update_installs_only_after_restart_prompt() {
        let mut a = app();
        let info = UpdateInfo::default();
        let _ = a.on_updates(Ux::Downloaded(Ok(Box::new(info))));
        // Downloaded: waiting for the user, nothing installed.
        assert!(matches!(a.updates.state, UpdateState::Ready(_)));
        // Later checks don't replace a ready update.
        let _ = a.on_updates(Ux::CheckNow);
        assert!(matches!(a.updates.state, UpdateState::Ready(_)));
        // A tick with consent doesn't install it either.
        a.settings.update_checks = Some(true);
        let _ = a.on_updates(Ux::Tick);
        assert!(matches!(a.updates.state, UpdateState::Ready(_)));
    }

    #[test]
    fn placeholder_key_turns_updates_off() {
        if RELEASE_KEY.contains("placeholder") {
            assert!(release_key().is_none());
            assert!(!Updates::start().available());
        } else {
            assert!(release_key().is_some());
        }
    }
}
