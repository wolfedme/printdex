//! Supervised background tasks: a long-running engine task that panics is
//! restarted instead of silently disappearing.
//!
//! [`spawn_supervised`] runs a fresh future from `make` each time. Its
//! panics are recorded as recovered (see
//! [`crate::diagnostics::recovering_future`]) and logged at `error`; the
//! task restarts after [`RESTART_BACKOFF`]. The third panic within
//! [`GIVE_UP_WINDOW`] stops it for good and publishes
//! [`EngineEvent::TaskStopped`]. A run that returns normally ends
//! supervision (the task was asked to stop).
//!
//! Whatever must survive a restart (queues, channels) lives outside the run
//! future, in what `make` clones into each run.

use std::any::Any;
use std::collections::VecDeque;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::diagnostics::{payload_message, recovering, recovering_future};
use crate::events::EngineEvent;

/// A supervised engine background task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskName {
    /// Writes `.printdex.toml` sidecars.
    SidecarWriter,
    /// Renders and caches thumbnails.
    Thumbnails,
    /// Watches libraries for changes and offline shares.
    Monitor,
    /// Runs unattended backups.
    BackupScheduler,
    /// Syncs on changes and on a timer.
    Sync,
}

/// Wait before the restart after a task's 1st, 2nd, and any later panic.
pub(crate) const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

/// This many panics within [`GIVE_UP_WINDOW`] stop the task.
const GIVE_UP_PANICS: usize = 3;

const GIVE_UP_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Spawns `make()`'s future and supervises it as described in
/// [the module docs](self). Aborting the returned handle stops supervision
/// and drops the current run.
pub(crate) fn spawn_supervised<F, Fut>(
    name: TaskName,
    events: broadcast::Sender<EngineEvent>,
    mut make: F,
) -> JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut recent: VecDeque<Instant> = VecDeque::new();
        let mut panics = 0usize;
        loop {
            let outcome = match std::panic::catch_unwind(AssertUnwindSafe(|| recovering(&mut make)))
            {
                Ok(run) => {
                    CatchUnwind {
                        inner: recovering_future(run),
                    }
                    .await
                }
                Err(payload) => Err(payload),
            };
            let Err(payload) = outcome else {
                return;
            };
            let message = payload_message(&*payload);
            let now = Instant::now();
            while recent
                .front()
                .is_some_and(|t| now.duration_since(*t) >= GIVE_UP_WINDOW)
            {
                recent.pop_front();
            }
            recent.push_back(now);
            panics += 1;
            if recent.len() >= GIVE_UP_PANICS {
                tracing::error!(task = ?name, %message, "background task panicked {GIVE_UP_PANICS} times in 10 minutes; stopping it");
                let _ = events.send(EngineEvent::TaskStopped(name));
                return;
            }
            let wait = RESTART_BACKOFF[(panics - 1).min(RESTART_BACKOFF.len() - 1)];
            tracing::error!(task = ?name, %message, restart_in = ?wait, "background task panicked; restarting it");
            tokio::time::sleep(wait).await;
        }
    })
}

pin_project_lite::pin_project! {
    /// Resolves to `Err` with the panic payload when polling `inner`
    /// panics. It must not be polled again after that.
    struct CatchUnwind<F> {
        #[pin]
        inner: F,
    }
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match std::panic::catch_unwind(AssertUnwindSafe(|| this.inner.poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(v)) => Poll::Ready(Ok(v)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn backs_off_then_gives_up() {
        let (tx, mut rx) = broadcast::channel(8);
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        let start = Instant::now();
        let handle = spawn_supervised(TaskName::Sync, tx, move || {
            r.fetch_add(1, Ordering::SeqCst);
            async { panic!("boom") }
        });
        handle.await.unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        assert_eq!(start.elapsed(), RESTART_BACKOFF[0] + RESTART_BACKOFF[1]);
        assert!(matches!(
            rx.try_recv(),
            Ok(EngineEvent::TaskStopped(TaskName::Sync))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn panics_far_apart_keep_restarting() {
        let (tx, mut rx) = broadcast::channel(8);
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        let start = Instant::now();
        let handle = spawn_supervised(TaskName::Monitor, tx, move || {
            let n = r.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 4 {
                    // Each run lasts 6 minutes, so at most two panics fall
                    // in any 10 minutes.
                    tokio::time::sleep(Duration::from_secs(6 * 60)).await;
                    panic!("boom {n}");
                }
            }
        });
        handle.await.unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 5, "a normal return ends it");
        let backoff: Duration = RESTART_BACKOFF.iter().sum::<Duration>() + RESTART_BACKOFF[2];
        assert_eq!(start.elapsed(), 4 * Duration::from_secs(6 * 60) + backoff);
        assert!(rx.try_recv().is_err(), "never stopped");
    }

    #[tokio::test]
    async fn normal_return_ends_supervision() {
        let (tx, mut rx) = broadcast::channel(8);
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        spawn_supervised(TaskName::Thumbnails, tx, move || {
            r.fetch_add(1, Ordering::SeqCst);
            async {}
        })
        .await
        .unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(rx.try_recv().is_err());
    }
}
