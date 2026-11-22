//! Matching libraries and models across machines (plan Task 6). Each
//! install gave its models its own ids, so a model another install
//! publishes is matched to a local one by where it is and what it holds.

use std::collections::{BTreeSet, HashMap};

use uuid::Uuid;

use super::state::Identity;
use crate::ids::ModelId;

/// Share of the smaller fingerprint set two libraries have in common at
/// which Printdex suggests linking them.
pub const SUGGEST_OVERLAP: f32 = 0.5;

/// Local models of one library, indexed for matching.
#[derive(Debug, Default)]
pub struct LocalIndex {
    by_path: HashMap<String, (ModelId, BTreeSet<u64>)>,
    by_fps: HashMap<Vec<u64>, Vec<ModelId>>,
}

impl LocalIndex {
    pub fn new(models: &[(ModelId, String, Vec<u64>)]) -> LocalIndex {
        let mut idx = LocalIndex::default();
        for (id, path, fps) in models {
            idx.by_path
                .insert(path.clone(), (*id, fps.iter().copied().collect()));
            if !fps.is_empty() {
                idx.by_fps.entry(fps.clone()).or_default().push(*id);
            }
        }
        idx
    }

    /// The local model `remote` is: 1) same path and at least one shared
    /// part fingerprint, else 2) exactly the same fingerprints somewhere
    /// else (a moved or renamed folder; only when that is unambiguous).
    pub fn match_model(&self, remote: &Identity) -> Option<ModelId> {
        if let Some((id, fps)) = self.by_path.get(&remote.path)
            && remote.fps.iter().any(|f| fps.contains(f))
        {
            return Some(*id);
        }
        if remote.fps.is_empty() {
            return None;
        }
        match self.by_fps.get(&remote.fps).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        }
    }
}

/// Which of two sync ids for the same model wins: the smaller.
pub fn winner(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Share of the smaller set that both hold (0 when either is empty).
pub fn library_overlap(a: &BTreeSet<u64>, b: &BTreeSet<u64>) -> f32 {
    let small = a.len().min(b.len());
    if small == 0 {
        return 0.0;
    }
    let common = if a.len() <= b.len() {
        a.iter().filter(|x| b.contains(x)).count()
    } else {
        b.iter().filter(|x| a.contains(x)).count()
    };
    common as f32 / small as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(n: u128) -> ModelId {
        ModelId(Uuid::from_u128(n))
    }

    fn idx() -> LocalIndex {
        LocalIndex::new(&[
            (m(1), "dragon".into(), vec![10, 11]),
            (m(2), "castle".into(), vec![20]),
            (m(3), "twin-a".into(), vec![30]),
            (m(4), "twin-b".into(), vec![30]),
        ])
    }

    fn remote(path: &str, fps: &[u64]) -> Identity {
        Identity {
            path: path.into(),
            fps: fps.to_vec(),
        }
    }

    #[test]
    fn same_path_matches() {
        assert_eq!(idx().match_model(&remote("dragon", &[11, 99])), Some(m(1)));
    }

    #[test]
    fn moved_folder_matches_by_fingerprints() {
        assert_eq!(
            idx().match_model(&remote("minis/dragon v2", &[10, 11])),
            Some(m(1))
        );
    }

    #[test]
    fn different_model_same_path_no_match() {
        assert_eq!(idx().match_model(&remote("dragon", &[77])), None);
        assert_eq!(idx().match_model(&remote("nowhere", &[])), None);
    }

    #[test]
    fn ambiguous_fingerprints_no_match() {
        assert_eq!(idx().match_model(&remote("elsewhere", &[30])), None);
        assert_eq!(idx().match_model(&remote("twin-b", &[30])), Some(m(4)));
    }

    #[test]
    fn double_publish_aliases_to_smaller() {
        let a = Uuid::from_u128(5);
        let b = Uuid::from_u128(9);
        assert_eq!(winner(b, a), (a, b));
        assert_eq!(winner(a, b), (a, b));
    }

    #[test]
    fn overlap_suggests_library() {
        let a: BTreeSet<u64> = (0..10).collect();
        let b: BTreeSet<u64> = (5..100).collect();
        assert!((library_overlap(&a, &b) - 0.5).abs() < 1e-6);
        assert!(library_overlap(&a, &b) >= SUGGEST_OVERLAP);
        assert_eq!(library_overlap(&a, &BTreeSet::new()), 0.0);
    }
}
