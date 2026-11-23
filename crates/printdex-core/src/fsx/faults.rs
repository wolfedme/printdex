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
//! - Dropping the [`FaultGuard`] disarms it.
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

#[cfg(any(test, feature = "fault-injection"))]
pub use armed::{Action, FaultGuard, arm, check};

#[cfg(any(test, feature = "fault-injection"))]
mod armed {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use super::Op;

    /// What an armed fault does to the operation it fires on.
    pub enum Action {
        /// The operation returns an error of this kind instead of running.
        Fail(io::ErrorKind),
        /// The closure runs with the operation's path, then the operation
        /// runs as usual (for example, to change a file mid-copy, or to
        /// create the target of a rename).
        Before(Box<dyn Fn(&Path) + Send + Sync>),
    }

    struct Armed {
        id: u64,
        root: PathBuf,
        op: Op,
        nth: usize,
        seen: usize,
        action: Arc<Action>,
        fired: Arc<AtomicUsize>,
    }

    static REGISTRY: Mutex<Vec<Armed>> = Mutex::new(Vec::new());
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn registry() -> MutexGuard<'static, Vec<Armed>> {
        // A panicking test must not take the others down with it.
        REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps a fault armed; dropping it disarms the fault.
    #[must_use = "the fault is disarmed as soon as the guard is dropped"]
    pub struct FaultGuard {
        id: u64,
        fired: Arc<AtomicUsize>,
    }

    impl FaultGuard {
        /// How many times this fault has fired so far (0 or 1).
        pub fn fired(&self) -> usize {
            self.fired.load(Ordering::SeqCst)
        }
    }

    impl Drop for FaultGuard {
        fn drop(&mut self) {
            registry().retain(|a| a.id != self.id);
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
        let firing: Vec<Arc<Action>> = {
            let mut reg = registry();
            reg.iter_mut()
                .filter(|a| a.op == op && path.starts_with(&a.root))
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
        for action in firing {
            match &*action {
                Action::Before(f) => f(path),
                Action::Fail(kind) => fail = fail.or(Some(*kind)),
            }
        }
        match fail {
            Some(kind) => Err(io::Error::new(
                kind,
                format!("injected fault: {op:?} at {}", path.display()),
            )),
            None => Ok(()),
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
