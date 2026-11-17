//! Thumbnails the app has received, and which on-screen items still need
//! one. The engine does the work; this only keeps image handles and asks
//! for what is visible.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use iced::widget::image;
use printdex_core::{ThumbFailure, ThumbKind, ThumbTarget};

/// Most thumbnails kept around. Each holds a decoded image only while iced
/// draws it, so this bounds bookkeeping, not texture memory.
const CAPACITY: usize = 4096;

/// What is known about one item's thumbnail.
#[derive(Clone, Debug)]
pub enum Thumb {
    Ready {
        handle: image::Handle,
        kind: ThumbKind,
    },
    Failed(ThumbFailure),
}

#[derive(Debug, Default)]
pub struct Thumbs {
    known: HashMap<ThumbTarget, Thumb>,
    /// Insertion order, oldest first, for eviction.
    order: VecDeque<ThumbTarget>,
    /// What was last asked of the engine.
    requested: Vec<ThumbTarget>,
    /// Shown, but asked for again (the model's material changed).
    stale: HashSet<ThumbTarget>,
}

impl Thumbs {
    pub fn get(&self, target: ThumbTarget) -> Option<&Thumb> {
        self.known.get(&target)
    }

    /// Whether `target` is still waiting for its first result.
    pub fn pending(&self, target: ThumbTarget) -> bool {
        !self.known.contains_key(&target) || self.stale.contains(&target)
    }

    /// Keeps showing `targets` but asks the engine again (it re-renders
    /// when the model's material changed, else answers from its cache).
    pub fn refresh(&mut self, targets: impl IntoIterator<Item = ThumbTarget>) {
        self.stale.extend(targets);
        self.requested.clear();
    }

    pub fn ready(&mut self, target: ThumbTarget, path: PathBuf, kind: ThumbKind) {
        self.insert(
            target,
            Thumb::Ready {
                handle: image::Handle::from_path(path),
                kind,
            },
        );
    }

    pub fn failed(&mut self, target: ThumbTarget, reason: ThumbFailure) {
        // A file that can't be read right now (offline share) is asked
        // for again next time it is on screen.
        if reason == ThumbFailure::Unreadable {
            return;
        }
        self.insert(target, Thumb::Failed(reason));
    }

    fn insert(&mut self, target: ThumbTarget, thumb: Thumb) {
        self.stale.remove(&target);
        if self.known.insert(target, thumb).is_none() {
            self.order.push_back(target);
        }
        while self.order.len() > CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.known.remove(&old);
            }
        }
    }

    /// Forgets everything (after a rescan, files may have changed).
    pub fn clear(&mut self) {
        self.known.clear();
        self.order.clear();
        self.requested.clear();
        self.stale.clear();
    }

    /// The visible items without a result, when that differs from the last
    /// request; `None` when nothing needs to be sent.
    pub fn request_for(&mut self, visible: Vec<ThumbTarget>) -> Option<Vec<ThumbTarget>> {
        let wanted: Vec<ThumbTarget> = visible.into_iter().filter(|t| self.pending(*t)).collect();
        if wanted == self.requested {
            return None;
        }
        self.requested = wanted.clone();
        Some(wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use printdex_core::ModelId;

    fn m(n: u8) -> ThumbTarget {
        ThumbTarget::Model(ModelId(uuid_from(n)))
    }

    fn uuid_from(n: u8) -> printdex_core::ids::Uuid {
        printdex_core::ids::Uuid::from_bytes([n; 16])
    }

    #[test]
    fn requests_only_missing_and_only_on_change() {
        let mut t = Thumbs::default();
        assert_eq!(t.request_for(vec![m(1), m(2)]), Some(vec![m(1), m(2)]));
        assert_eq!(t.request_for(vec![m(1), m(2)]), None);
        t.ready(m(1), "/x/p1.png".into(), ThumbKind::Render);
        assert_eq!(t.request_for(vec![m(1), m(2)]), Some(vec![m(2)]));
        t.failed(m(2), ThumbFailure::Invalid);
        assert_eq!(t.request_for(vec![m(1), m(2)]), Some(vec![]));
        assert_eq!(t.request_for(vec![m(1), m(2)]), None);
    }

    #[test]
    fn refreshed_thumbs_stay_shown_and_are_asked_again() {
        let mut t = Thumbs::default();
        t.ready(m(1), "/x/p1.png".into(), ThumbKind::Render);
        assert_eq!(t.request_for(vec![m(1)]), None);
        t.refresh([m(1)]);
        assert!(t.get(m(1)).is_some());
        assert_eq!(t.request_for(vec![m(1)]), Some(vec![m(1)]));
        t.ready(m(1), "/x/p2.png".into(), ThumbKind::Render);
        assert_eq!(t.request_for(vec![m(1)]), Some(vec![]));
    }

    #[test]
    fn unreadable_is_asked_again() {
        let mut t = Thumbs::default();
        t.failed(m(1), ThumbFailure::Unreadable);
        assert!(t.pending(m(1)));
    }

    #[test]
    fn evicts_oldest_beyond_capacity() {
        let mut t = Thumbs::default();
        for i in 0..=CAPACITY {
            let id = ThumbTarget::Part(printdex_core::PartId(printdex_core::ids::Uuid::from_u128(
                i as u128,
            )));
            t.failed(id, ThumbFailure::Invalid);
        }
        assert_eq!(t.known.len(), CAPACITY);
        let first = ThumbTarget::Part(printdex_core::PartId(printdex_core::ids::Uuid::from_u128(
            0,
        )));
        assert!(t.pending(first));
    }
}
