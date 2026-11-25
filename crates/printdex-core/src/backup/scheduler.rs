//! Unattended backups while the app is open.
//!
//! - Data: 5 minutes after the last change to user data, and at least
//!   every 24 hours.
//! - Models: daily, weekly or only by hand, per the schedule.
//! - Prune at most weekly, a check reading 2 % of the data monthly.
//! - Nothing in the first minute after launch, nothing while a scan runs.
//! - Transient errors (target unreachable) retry after 1, 5 and 30
//!   minutes, then hourly; wrong credentials or passphrase wait for the
//!   user.
//!
//! [`due`] and [`attention`] are pure functions of [`SchedState`], so the
//! timings are tested without waiting.

use std::collections::{HashMap, HashSet};
use std::sync::PoisonError;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::config::ModelSchedule;
use super::{Attention, BackupKind, BackupScope};
use crate::engine::{Engine, LibraryStatus, folder_status};
use crate::error::{BackupError, CoreError, Result};
use crate::events::EngineEvent;
use crate::ids::LibraryId;
use crate::supervise::{TaskName, spawn_supervised};
use crate::time::now_ms;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Quiet time after a change before the data backup.
pub const DATA_DEBOUNCE: i64 = 5 * MINUTE;
/// Longest time between data backups.
pub const DATA_EVERY: i64 = DAY;
pub const PRUNE_EVERY: i64 = 7 * DAY;
pub const CHECK_EVERY: i64 = 30 * DAY;
/// Share of the stored data the monthly check reads back.
pub const CHECK_READ_PERCENT: f64 = 2.0;
/// No data backup for this long needs attention.
pub const DATA_STALE: i64 = 3 * DAY;
/// No model backup of a library for this long needs attention.
pub const MODELS_STALE: i64 = 14 * DAY;
/// Consecutive transient failures before "target unreachable" shows.
pub const UNREACHABLE_AFTER: u32 = 4;

/// A scheduled job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Data,
    Models(LibraryId),
    Prune,
    Check,
}

/// One library, as the scheduler sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibState {
    pub id: LibraryId,
    /// Model backup on.
    pub on: bool,
    pub online: bool,
    pub last_ok: Option<i64>,
}

/// Everything the schedule depends on. Times are unix milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct SchedState {
    /// A target with known secrets is set up.
    pub configured: bool,
    /// When the backup was set up (staleness counts from there before the
    /// first backup).
    pub configured_at: i64,
    pub launched_at: i64,
    pub launch_grace: i64,
    pub scanning: bool,
    pub data_changed_at: Option<i64>,
    pub last_data_ok: Option<i64>,
    pub models: ModelSchedule,
    pub libraries: Vec<LibState>,
    pub last_prune: Option<i64>,
    pub last_check: Option<i64>,
    /// No jobs before this (retry backoff).
    pub retry_at: Option<i64>,
    /// Consecutive transient failures.
    pub failures: u32,
    /// A problem only the user can fix (credentials, passphrase, full
    /// target, failed check): jobs wait until the settings change.
    pub blocked: Option<Attention>,
}

impl SchedState {
    pub fn new(now: i64) -> SchedState {
        SchedState {
            configured: false,
            configured_at: now,
            launched_at: now,
            launch_grace: MINUTE,
            scanning: false,
            data_changed_at: None,
            last_data_ok: None,
            models: ModelSchedule::Daily,
            libraries: Vec::new(),
            last_prune: None,
            last_check: None,
            retry_at: None,
            failures: 0,
            blocked: None,
        }
    }
}

fn older_than(last: Option<i64>, now: i64, span: i64) -> bool {
    last.is_none_or(|t| now - t >= span)
}

/// The jobs due at `now`, in the order to run them.
pub fn due(now: i64, s: &SchedState) -> Vec<Job> {
    if !s.configured
        || s.scanning
        || s.blocked.is_some()
        || now - s.launched_at < s.launch_grace
        || s.retry_at.is_some_and(|t| now < t)
    {
        return Vec::new();
    }
    let mut jobs = Vec::new();
    let changed = s
        .data_changed_at
        .is_some_and(|c| s.last_data_ok.is_none_or(|ok| c > ok) && now - c >= DATA_DEBOUNCE);
    if changed || older_than(s.last_data_ok, now, DATA_EVERY) {
        jobs.push(Job::Data);
    }
    let period = match s.models {
        ModelSchedule::Daily => Some(DAY),
        ModelSchedule::Weekly => Some(7 * DAY),
        ModelSchedule::Manual => None,
    };
    if let Some(period) = period {
        for lib in &s.libraries {
            if lib.on && lib.online && older_than(lib.last_ok, now, period) {
                jobs.push(Job::Models(lib.id));
            }
        }
    }
    // Maintenance only once there is something to maintain.
    if s.last_data_ok.is_some() {
        if older_than(s.last_prune, now, PRUNE_EVERY) {
            jobs.push(Job::Prune);
        }
        if older_than(s.last_check, now, CHECK_EVERY) {
            jobs.push(Job::Check);
        }
    }
    jobs
}

/// What needs the user's attention at `now`.
pub fn attention(now: i64, s: &SchedState) -> Vec<Attention> {
    let mut out = Vec::new();
    if !s.configured {
        // Unconfigured: nothing is promised, nothing to nag about, unless
        // secrets went missing for a configured target.
        if let Some(b) = &s.blocked {
            out.push(b.clone());
        }
        return out;
    }
    if let Some(b) = &s.blocked {
        out.push(b.clone());
    }
    if s.failures >= UNREACHABLE_AFTER {
        out.push(Attention::TargetUnreachable);
    }
    let since = |last: Option<i64>| now - last.unwrap_or(s.configured_at);
    if since(s.last_data_ok) > DATA_STALE {
        out.push(Attention::DataStale);
    }
    for lib in s.libraries.iter().filter(|l| l.on) {
        if since(lib.last_ok) > MODELS_STALE {
            out.push(if lib.online {
                Attention::ModelsStale(lib.id)
            } else {
                Attention::ConnectLibrary(lib.id)
            });
        }
    }
    out
}

/// Wait before the next try after `failures` transient failures.
pub fn backoff(failures: u32) -> i64 {
    match failures {
        0 => 0,
        1 => MINUTE,
        2 => 5 * MINUTE,
        3 => 30 * MINUTE,
        _ => HOUR,
    }
}

/// Timings of the running scheduler (tests shrink them).
#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    /// How often the schedule is looked at.
    pub tick: Duration,
    /// Quiet time after launch, in milliseconds.
    pub launch_grace: i64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            tick: Duration::from_secs(30),
            launch_grace: MINUTE,
        }
    }
}

/// The running scheduler. Dropping it stops it and cancels a running
/// backup, so quitting never waits for an upload.
#[derive(Debug)]
pub struct BackupScheduler {
    stop: CancellationToken,
    engine: Engine,
}

impl Drop for BackupScheduler {
    fn drop(&mut self) {
        self.stop.cancel();
        self.engine.cancel_backup();
    }
}

/// Whether an error is worth retrying on its own.
fn transient(e: &CoreError) -> bool {
    matches!(
        e,
        CoreError::Backup(BackupError::Unreachable(_) | BackupError::Repository(_))
            | CoreError::Io { .. }
    )
}

/// The attention an error needs right away, if any.
fn blocking(e: &CoreError) -> Option<Attention> {
    match e {
        CoreError::Backup(BackupError::WrongPassphrase | BackupError::SecretsNeeded) => {
            Some(Attention::PassphraseNeeded)
        }
        CoreError::Backup(BackupError::Credentials | BackupError::ReadOnly) => {
            Some(Attention::Credentials)
        }
        CoreError::Backup(BackupError::Full) => Some(Attention::TargetFull),
        CoreError::Backup(BackupError::CheckFailed(_)) => Some(Attention::CheckFailed),
        _ => None,
    }
}

/// Events that change what the data snapshot would hold.
fn changes_data(e: &EngineEvent) -> bool {
    matches!(
        e,
        EngineEvent::ModelDataChanged(_)
            | EngineEvent::OrganizeChanged
            | EngineEvent::ModelForgotten(_)
            | EngineEvent::FileOpDone(_)
            | EngineEvent::LibraryAdded(_)
            | EngineEvent::LibraryRemoved(_)
            | EngineEvent::LibraryRenamed(_)
            | EngineEvent::LibraryChanged(_)
    )
}

impl Engine {
    /// Starts unattended backups. Drop the handle to stop them.
    pub fn start_backup_scheduler(&self) -> BackupScheduler {
        self.start_backup_scheduler_with(SchedulerConfig::default())
    }

    pub fn start_backup_scheduler_with(&self, config: SchedulerConfig) -> BackupScheduler {
        let stop = CancellationToken::new();
        let (engine, token) = (self.clone(), stop.clone());
        spawn_supervised(TaskName::BackupScheduler, self.events.clone(), move || {
            schedule(engine.clone(), config.clone(), token.clone())
        });
        BackupScheduler {
            stop,
            engine: self.clone(),
        }
    }

    /// What needs the user's attention, as last worked out by the
    /// scheduler (empty before it ran).
    pub fn backup_attention(&self) -> Vec<Attention> {
        self.backup
            .attention
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Fills `s` from the database: settings, last runs, library states.
    async fn refresh_sched(
        &self,
        s: &mut SchedState,
        online: &HashMap<LibraryId, bool>,
    ) -> Result<()> {
        let config = self.backup_config().await?;
        let target = config.targets.first();
        let had_config = s.configured;
        s.configured = target.is_some_and(|t| self.target_secrets_known(t));
        if target.is_some() && !s.configured {
            s.blocked = Some(Attention::PassphraseNeeded);
        } else if s.configured && !had_config {
            // Secrets arrived or the target changed: try again.
            s.blocked = None;
            s.failures = 0;
            s.retry_at = None;
        }
        if let Some(t) = target {
            s.configured_at = self
                .db
                .target_created_at(t.id)
                .await?
                .unwrap_or(s.configured_at);
        }
        s.models = config.schedule.models;
        s.last_data_ok = self.db.last_ok_run("data", None).await?;
        s.last_prune = self.db.last_ok_run("prune", None).await?;
        s.last_check = self.db.last_ok_run("check", None).await?;
        let mut libs = Vec::new();
        for lib in self.db.libraries().await? {
            libs.push(LibState {
                id: lib.id,
                on: config.models_on(lib.id),
                online: online.get(&lib.id).copied().unwrap_or(true),
                last_ok: self.db.last_ok_run("models", Some(lib.id)).await?,
            });
        }
        s.libraries = libs;
        Ok(())
    }

    /// Runs one scheduled job.
    async fn run_scheduled(&self, job: Job) -> Result<()> {
        match job {
            Job::Data => self.backup_now(BackupScope::Data).await.map(|_| ()),
            Job::Models(lib) => self.backup_now(BackupScope::Models(lib)).await.map(|_| ()),
            Job::Prune => self
                .run_job(BackupKind::Prune, |repo, _, _| {
                    let freed = repo.prune()?;
                    tracing::info!(freed, "pruned the backup repository");
                    Ok(None)
                })
                .await
                .map(|_| ()),
            Job::Check => self
                .run_job(BackupKind::Check, |repo, _, _| {
                    repo.check(CHECK_READ_PERCENT)?;
                    Ok(None)
                })
                .await
                .map(|_| ()),
        }
    }

    fn publish_attention(&self, now: Vec<Attention>) {
        let mut cur = self
            .backup
            .attention
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *cur != now {
            cur.clone_from(&now);
            drop(cur);
            let _ = self.events.send(EngineEvent::BackupNeedsAttention(now));
        }
    }
}

async fn schedule(engine: Engine, config: SchedulerConfig, stop: CancellationToken) {
    let mut events = engine.subscribe();
    if let Err(e) = engine.db.close_stale_runs().await {
        tracing::warn!(error = %e, "could not close interrupted backup runs");
    }
    let mut s = SchedState::new(now_ms());
    s.launch_grace = config.launch_grace;
    // Changes since the last data backup survive a restart through the
    // models' update times.
    s.data_changed_at = engine.db.last_user_change().await.ok().flatten();
    let mut scanning: HashSet<LibraryId> = HashSet::new();
    let mut online: HashMap<LibraryId, bool> = HashMap::new();
    for lib in engine.db.libraries().await.unwrap_or_default() {
        let up = folder_status(engine.fs.clone(), lib.path.clone()).await == LibraryStatus::Online;
        online.insert(lib.id, up);
    }
    let mut tick = tokio::time::interval(config.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        crate::fsx::faults::checkpoint(TaskName::BackupScheduler);
        tokio::select! {
            () = stop.cancelled() => break,
            _ = tick.tick() => {}
            ev = events.recv() => {
                match ev {
                    Ok(e) => {
                        if changes_data(&e) {
                            s.data_changed_at = Some(now_ms());
                        }
                        match e {
                            EngineEvent::ScanStarted { library } => { scanning.insert(library); }
                            EngineEvent::ScanFinished { library, .. }
                            | EngineEvent::ScanFailed { library, .. } => { scanning.remove(&library); }
                            EngineEvent::LibraryOnline(l) => { online.insert(l, true); }
                            EngineEvent::LibraryOffline(l) => { online.insert(l, false); }
                            _ => {}
                        }
                    }
                    Err(RecvError::Lagged(_)) => s.data_changed_at = Some(now_ms()),
                    Err(RecvError::Closed) => break,
                }
                continue;
            }
        }
        s.scanning = !scanning.is_empty();
        if let Err(e) = engine.refresh_sched(&mut s, &online).await {
            tracing::warn!(error = %e, "backup scheduler could not read its state");
            continue;
        }
        for job in due(now_ms(), &s) {
            if stop.is_cancelled() || !scanning.is_empty() {
                break;
            }
            match engine.run_scheduled(job).await {
                Ok(()) => {
                    s.failures = 0;
                    s.retry_at = None;
                }
                Err(CoreError::Cancelled) => break,
                Err(e) => {
                    tracing::warn!(error = %e, ?job, "scheduled backup failed");
                    if let Some(a) = blocking(&e) {
                        s.blocked = Some(a);
                    } else if transient(&e) {
                        s.failures += 1;
                        s.retry_at = Some(now_ms() + backoff(s.failures));
                    }
                    break;
                }
            }
            // Refresh "last ok" times before the next job and attention.
            let _ = engine.refresh_sched(&mut s, &online).await;
        }
        engine.publish_attention(attention(now_ms(), &s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn state() -> SchedState {
        let mut s = SchedState::new(NOW - DAY);
        s.configured = true;
        s.last_data_ok = Some(NOW - HOUR);
        s.last_prune = Some(NOW - HOUR);
        s.last_check = Some(NOW - HOUR);
        s
    }

    #[test]
    fn data_backup_debounced_5_min() {
        let mut s = state();
        assert!(due(NOW, &s).is_empty());
        s.data_changed_at = Some(NOW - 4 * MINUTE);
        assert!(due(NOW, &s).is_empty(), "still within the quiet time");
        s.data_changed_at = Some(NOW - 5 * MINUTE);
        assert_eq!(due(NOW, &s), [Job::Data]);
        // A change older than the last backup is already in it.
        s.data_changed_at = Some(NOW - 2 * HOUR);
        assert!(due(NOW, &s).is_empty());
        // At least daily, change or not.
        s.data_changed_at = None;
        s.last_data_ok = Some(NOW - DAY);
        assert_eq!(due(NOW, &s), [Job::Data]);
    }

    #[test]
    fn due_on_launch_runs_soon() {
        let mut s = state();
        s.last_data_ok = Some(NOW - 3 * DAY);
        s.launched_at = NOW - 10_000;
        assert!(due(NOW, &s).is_empty(), "not in the first minute");
        s.launched_at = NOW - MINUTE;
        assert_eq!(due(NOW, &s), [Job::Data]);
        // Never backed up: due right after the grace too.
        s.last_data_ok = None;
        assert_eq!(due(NOW, &s), [Job::Data]);
    }

    #[test]
    fn models_follow_their_schedule() {
        let lib = LibraryId::new();
        let mut s = state();
        s.libraries = vec![LibState {
            id: lib,
            on: true,
            online: true,
            last_ok: Some(NOW - 2 * DAY),
        }];
        assert_eq!(due(NOW, &s), [Job::Models(lib)]);
        s.models = ModelSchedule::Weekly;
        assert!(due(NOW, &s).is_empty());
        s.libraries[0].last_ok = Some(NOW - 8 * DAY);
        assert_eq!(due(NOW, &s), [Job::Models(lib)]);
        s.models = ModelSchedule::Manual;
        assert!(due(NOW, &s).is_empty());
        s.models = ModelSchedule::Daily;
        s.libraries[0].on = false;
        assert!(due(NOW, &s).is_empty());
    }

    #[test]
    fn offline_library_skipped_not_failed() {
        let lib = LibraryId::new();
        let mut s = state();
        s.libraries = vec![LibState {
            id: lib,
            on: true,
            online: false,
            last_ok: Some(NOW - 10 * DAY),
        }];
        assert!(due(NOW, &s).is_empty(), "no job for an offline library");
        assert!(attention(NOW, &s).is_empty(), "no message before 14 days");
        s.libraries[0].last_ok = Some(NOW - 15 * DAY);
        assert_eq!(attention(NOW, &s), [Attention::ConnectLibrary(lib)]);
        s.libraries[0].online = true;
        assert_eq!(attention(NOW, &s), [Attention::ModelsStale(lib)]);
    }

    #[test]
    fn attention_after_thresholds_only() {
        let mut s = state();
        s.last_data_ok = Some(NOW - 2 * DAY);
        assert!(attention(NOW, &s).is_empty());
        s.last_data_ok = Some(NOW - 3 * DAY - 1);
        assert_eq!(attention(NOW, &s), [Attention::DataStale]);
        // Never backed up: counted from setup.
        s.last_data_ok = None;
        s.configured_at = NOW - DAY;
        assert!(attention(NOW, &s).is_empty());
        // Transient failures show only once they persist.
        s.failures = UNREACHABLE_AFTER - 1;
        assert!(attention(NOW, &s).is_empty());
        s.failures = UNREACHABLE_AFTER;
        assert_eq!(attention(NOW, &s), [Attention::TargetUnreachable]);
        // Not configured at all: no nagging.
        let fresh = SchedState::new(NOW - 30 * DAY);
        assert!(attention(NOW, &fresh).is_empty());
    }

    #[test]
    fn scheduler_yields_while_scanning() {
        let mut s = state();
        s.last_data_ok = None;
        s.scanning = true;
        assert!(due(NOW, &s).is_empty());
        s.scanning = false;
        assert_eq!(due(NOW, &s), [Job::Data]);
    }

    #[test]
    fn blocked_and_backoff_wait() {
        let mut s = state();
        s.last_data_ok = Some(NOW - 2 * DAY);
        s.blocked = Some(Attention::PassphraseNeeded);
        assert!(due(NOW, &s).is_empty());
        assert_eq!(attention(NOW, &s), [Attention::PassphraseNeeded]);
        s.blocked = None;
        s.retry_at = Some(NOW + 1);
        assert!(due(NOW, &s).is_empty());
        s.retry_at = Some(NOW);
        assert_eq!(due(NOW, &s), [Job::Data]);
        assert_eq!(
            [1, 2, 3, 4, 9].map(backoff),
            [MINUTE, 5 * MINUTE, 30 * MINUTE, HOUR, HOUR]
        );
    }

    #[test]
    fn maintenance_is_weekly_and_monthly() {
        let mut s = state();
        s.last_prune = Some(NOW - 7 * DAY);
        assert_eq!(due(NOW, &s), [Job::Prune]);
        s.last_prune = Some(NOW);
        s.last_check = Some(NOW - 30 * DAY);
        assert_eq!(due(NOW, &s), [Job::Check]);
        // Nothing to maintain before the first backup.
        s.last_data_ok = None;
        s.last_check = None;
        s.launched_at = NOW - DAY;
        assert_eq!(due(NOW, &s), [Job::Data]);
    }
}
