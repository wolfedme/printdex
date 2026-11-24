//! Fault injection for [`crate::fsx`]: tests fail, or interfere with, the
//! n-th file operation under a folder.
//!
//! Every `fsx` function calls [`check`] *before* each real operation. With
//! the `fault-injection` feature off (release builds) `check` is an inlined
//! no-op. With it on (this crate's own unit tests, and `tests/it` through the
//! self dev-dependency), [`arm`] registers a fault in a process-global list:
//!
//! - It only matches paths that start with its `root`, so tests running in
//!   parallel in different temporary folders don't see each other's faults.
//! - `nth` is 0-based and counts only operations of its [`Op`] under its
//!   `root`, separately for each guard. It fires once, on exactly that one.
//! - [`Action::Fail`] makes that operation return an error of that kind
//!   instead of running. [`Action::Before`] calls the closure with the path,
//!   then lets the real operation run.
//! - [`Action::Instead`] runs the closure *in place of* the operation, at
//!   call sites that go through [`perform`] (moving to the trash and
//!   removing, in file actions), so tests can keep the system trash out of
//!   it. Where a call site only uses [`check`], it still counts and
//!   reports as fired, but is dropped: the closure never runs and the real
//!   operation runs as usual. A `Fail` firing on the same operation wins
//!   over it.
//! - Dropping the [`FaultGuard`] disarms it.
//!
//! Supervised background tasks (see `crate::supervise`) call [`checkpoint`]
//! once per loop iteration. [`arm_panic`] makes the next `times` checkpoints
//! of one [`TaskName`] panic, but only on the tokio runtime that armed it,
//! so parallel tests (each on its own runtime) don't see each other's
//! panics.
//!
//! Which path each operation passes:
//!
//! - [`Op::Write`]: the final path being written (not its temporary file).
//! - [`Op::Rename`]: the destination `to`.
//! - [`Op::Copy`]: the destination file about to be written.
//! - [`Op::Remove`]: the entry about to be removed.
//! - [`Op::Trash`]: the entry about to be moved to the trash.

/// A kind of file operation that [`check`] can intercept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// Writing a file's contents ([`crate::fsx::durable_write`]).
    Write,
    /// Renaming or moving an entry.
    Rename,
    /// Copying one file (each file of [`crate::fsx::copy_tree_verified`]).
    Copy,
    /// Deleting an entry.
    Remove,
    /// Moving an entry to the system trash.
    Trash,
}

/// Called before each file operation that `fsx` performs, with the path
/// that [the module docs](self) list for `op`. Returns the injected error
/// when an armed [`Action::Fail`] fires; callers return it instead of doing
/// the operation.
#[cfg(not(any(test, feature = "fault-injection")))]
#[inline]
pub fn check(_op: Op, _path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

/// Called once per loop iteration of each supervised task. Panics while an
/// [`arm_panic`] for `task` is armed on this runtime.
#[cfg(not(any(test, feature = "fault-injection")))]
#[inline]
pub fn checkpoint(_task: TaskName) {}

/// Runs `real`, the operation `op` on `path`, after [`check`]; an armed
/// [`Action::Instead`] that fires replaces it.
#[cfg(not(any(test, feature = "fault-injection")))]
#[inline]
pub fn perform(
    _op: Op,
    _path: &std::path::Path,
    real: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    real()
}

#[cfg(any(test, feature = "fault-injection"))]
pub use armed::{Action, FaultGuard, InsteadFn, arm, arm_panic, check, checkpoint, perform};

use crate::supervise::TaskName;

#[cfg(any(test, feature = "fault-injection"))]
mod armed {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use super::{Op, TaskName};

    /// The closure of an [`Action::Instead`].
    pub type InsteadFn = dyn Fn(&Path) -> io::Result<()> + Send + Sync;

    /// What an armed fault does to the operation it fires on.
    pub enum Action {
        /// The operation returns an error of this kind instead of running.
        Fail(io::ErrorKind),
        /// The closure runs with the operation's path, then the operation
        /// runs as usual (for example, to change a file mid-copy, or to
        /// create the target of a rename).
        Before(Box<dyn Fn(&Path) + Send + Sync>),
        /// The closure runs with the operation's path *instead of* the
        /// operation, and its result is the operation's, at call sites that
        /// use [`perform`]; elsewhere ([`check`] only) it is dropped: the
        /// closure never runs and the operation runs as usual.
        Instead(Box<InsteadFn>),
    }

    struct Armed {
        id: u64,
        root: PathBuf,
        /// `root` resolved the way the engine stores library paths: on
        /// Windows a temp folder can be given by its 8.3 short name
        /// (`RUNNER~1`) while the engine works with the long name.
        canonical: Option<PathBuf>,
        op: Op,
        nth: usize,
        seen: usize,
        action: Arc<Action>,
        fired: Arc<AtomicUsize>,
    }

    /// An armed [`arm_panic`].
    struct ArmedPanic {
        id: u64,
        task: TaskName,
        runtime: Option<tokio::runtime::Id>,
        left: usize,
        fired: Arc<AtomicUsize>,
    }

    static REGISTRY: Mutex<Vec<Armed>> = Mutex::new(Vec::new());
    static PANICS: Mutex<Vec<ArmedPanic>> = Mutex::new(Vec::new());
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn registry() -> MutexGuard<'static, Vec<Armed>> {
        // A panicking test must not take the others down with it.
        REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn panics() -> MutexGuard<'static, Vec<ArmedPanic>> {
        PANICS.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn current_runtime() -> Option<tokio::runtime::Id> {
        tokio::runtime::Handle::try_current().ok().map(|h| h.id())
    }

    /// Makes the next `times` [`checkpoint`]s of `task` on the current
    /// tokio runtime panic, until the returned guard is dropped.
    pub fn arm_panic(task: TaskName, times: usize) -> FaultGuard {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let fired = Arc::new(AtomicUsize::new(0));
        panics().push(ArmedPanic {
            id,
            task,
            runtime: current_runtime(),
            left: times,
            fired: fired.clone(),
        });
        FaultGuard { id, fired }
    }

    /// Called once per loop iteration of each supervised task. Panics while
    /// an [`arm_panic`] for `task` is armed on this runtime.
    #[expect(
        clippy::panic,
        reason = "fault injection: the panic is the injected fault, and this module compiles only with the fault-injection feature"
    )]
    pub fn checkpoint(task: TaskName) {
        let runtime = current_runtime();
        let hit = panics()
            .iter_mut()
            .find(|p| p.task == task && p.runtime == runtime && p.left > 0)
            .map(|p| {
                p.left -= 1;
                p.fired.fetch_add(1, Ordering::SeqCst);
            })
            .is_some();
        // Outside the lock, so the registry is not poisoned.
        if hit {
            panic!("injected panic in {task:?}");
        }
    }

    /// Keeps a fault armed; dropping it disarms the fault.
    #[must_use = "the fault is disarmed as soon as the guard is dropped"]
    pub struct FaultGuard {
        id: u64,
        fired: Arc<AtomicUsize>,
    }

    impl FaultGuard {
        /// How many times this fault has fired so far (0 or 1, or up to
        /// `times` for [`arm_panic`]).
        pub fn fired(&self) -> usize {
            self.fired.load(Ordering::SeqCst)
        }
    }

    impl Drop for FaultGuard {
        fn drop(&mut self) {
            registry().retain(|a| a.id != self.id);
            panics().retain(|p| p.id != self.id);
        }
    }

    /// Arms a fault on the `nth` (0-based) operation of kind `op` whose path
    /// starts with `root`, until the returned guard is dropped.
    pub fn arm(root: &Path, op: Op, nth: usize, action: Action) -> FaultGuard {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let fired = Arc::new(AtomicUsize::new(0));
        registry().push(Armed {
            id,
            root: root.to_path_buf(),
            canonical: dunce::canonicalize(root).ok(),
            op,
            nth,
            seen: 0,
            action: Arc::new(action),
            fired: fired.clone(),
        });
        FaultGuard { id, fired }
    }

    /// Called before each file operation that `fsx` performs, with the path
    /// that [the module docs](super) list for `op`. Counts the operation for
    /// every matching armed fault, runs any [`Action::Before`] that fires,
    /// and returns the error of an [`Action::Fail`] that fires; callers
    /// return it instead of doing the operation.
    pub fn check(op: Op, path: &Path) -> io::Result<()> {
        fire(op, path).map(|_| ())
    }

    /// Runs `real`, the operation `op` on `path`, after the checks of
    /// [`check`]; an [`Action::Instead`] that fires runs in its place.
    pub fn perform(op: Op, path: &Path, real: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        match fire(op, path)? {
            Some(instead) => match &*instead {
                Action::Instead(f) => f(path),
                _ => unreachable!("fire only returns Instead actions"),
            },
            None => real(),
        }
    }

    /// Counts the operation, runs `Before` closures, and returns a `Fail`
    /// error, or else the first `Instead` that fired.
    fn fire(op: Op, path: &Path) -> io::Result<Option<Arc<Action>>> {
        let firing: Vec<Arc<Action>> = {
            let mut reg = registry();
            reg.iter_mut()
                .filter(|a| {
                    a.op == op
                        && (path.starts_with(&a.root)
                            || a.canonical.as_ref().is_some_and(|c| path.starts_with(c)))
                })
                .filter_map(|a| {
                    let hit = a.seen == a.nth;
                    a.seen += 1;
                    hit.then(|| {
                        a.fired.fetch_add(1, Ordering::SeqCst);
                        a.action.clone()
                    })
                })
                .collect()
        };
        // Outside the lock: a `Before` closure may do file work of its own.
        let mut fail = None;
        let mut instead = None;
        for action in firing {
            match &*action {
                Action::Before(f) => f(path),
                Action::Fail(kind) => fail = fail.or(Some(*kind)),
                Action::Instead(_) => {
                    if instead.is_none() {
                        instead = Some(action.clone());
                    }
                }
            }
        }
        match fail {
            Some(kind) => Err(io::Error::new(
                kind,
                format!("injected fault: {op:?} at {}", path.display()),
            )),
            None => Ok(instead),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn counts_per_root_and_op_and_disarms_on_drop() {
            let a = Path::new("/faults-test/a");
            let b = Path::new("/faults-test/b");
            let g = arm(a, Op::Write, 1, Action::Fail(io::ErrorKind::StorageFull));
            // Other roots and other ops don't count.
            check(Op::Write, &b.join("x")).unwrap();
            check(Op::Rename, &a.join("x")).unwrap();
            check(Op::Write, &a.join("x")).unwrap(); // nth 0
            assert_eq!(g.fired(), 0);
            let err = check(Op::Write, &a.join("y")).unwrap_err(); // nth 1
            assert_eq!(err.kind(), io::ErrorKind::StorageFull);
            assert_eq!(g.fired(), 1);
            check(Op::Write, &a.join("z")).unwrap(); // fires once only
            assert_eq!(g.fired(), 1);
            drop(g);
            let g = arm(a, Op::Write, 0, Action::Before(Box::new(|_| {})));
            check(Op::Write, &a.join("x")).unwrap();
            assert_eq!(g.fired(), 1);
            drop(g);
            assert!(
                registry()
                    .iter()
                    .all(|x| !x.root.starts_with("/faults-test"))
            );
        }
    }
}
