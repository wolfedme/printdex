//! Model identity across moves and renames: a model that disappeared from
//! one path and appeared at another keeps its id (and with it tags, notes,
//! recipe, print log, catalogue number and thumbnails), matched by the
//! fingerprints of its parts.

use std::collections::HashMap;

use crate::grouping::ModelKind;

/// A part's content identity: size and partial-content hash.
pub type Key = (u64, u64);

/// A model that the scan did not find at its old path.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub kind: ModelKind,
    pub keys: Vec<Key>,
}

/// A model the scan found at a path with no model row.
#[derive(Clone, Debug)]
pub struct Arrival {
    pub kind: ModelKind,
    /// `None` for a part that could not be fingerprinted.
    pub keys: Vec<Option<Key>>,
}

/// Share of parts that must match on both sides for a folder model.
const MIN_SHARE: f64 = 0.8;

/// Pairs arrivals with the candidates they are. Returns
/// `arrival index → candidate index`. A pairing is made only when it is
/// unambiguous: one best candidate for the arrival, and no other arrival
/// claiming the same candidate. Duplicate copies therefore never merge.
pub fn match_moves(arrivals: &[Arrival], candidates: &[Candidate]) -> HashMap<usize, usize> {
    let mut by_key: HashMap<Key, Vec<usize>> = HashMap::new();
    for (c, cand) in candidates.iter().enumerate() {
        for k in &cand.keys {
            let list = by_key.entry(*k).or_default();
            if list.last() != Some(&c) {
                list.push(c);
            }
        }
    }

    // Best candidate per arrival.
    let mut best: HashMap<usize, usize> = HashMap::new();
    for (a, arr) in arrivals.iter().enumerate() {
        if arr.keys.is_empty() || arr.keys.iter().any(Option::is_none) {
            continue;
        }
        let mut hits: HashMap<usize, usize> = HashMap::new();
        for k in arr.keys.iter().flatten() {
            for &c in by_key.get(k).map(Vec::as_slice).unwrap_or_default() {
                *hits.entry(c).or_default() += 1;
            }
        }
        let mut scored: Vec<(usize, f64)> = hits
            .into_iter()
            .filter_map(|(c, n)| {
                let cand = candidates.get(c).filter(|cand| cand.kind == arr.kind)?;
                let old_share = n as f64 / cand.keys.len() as f64;
                let new_share = n as f64 / arr.keys.len() as f64;
                let ok = match arr.kind {
                    ModelKind::SingleFile => n == 1 && cand.keys.len() == 1 && arr.keys.len() == 1,
                    ModelKind::Folder => old_share >= MIN_SHARE && new_share >= MIN_SHARE,
                };
                ok.then_some((c, old_share.min(new_share)))
            })
            .collect();
        scored.sort_by(|x, y| y.1.total_cmp(&x.1));
        match scored.as_slice() {
            [] => {}
            [(c, _)] => {
                best.insert(a, *c);
            }
            [(c, s1), (_, s2), ..] if s1 > s2 => {
                best.insert(a, *c);
            }
            // Two equally good candidates: leave both alone.
            _ => {}
        }
    }

    // A candidate claimed by two arrivals (a copy next to the move) is
    // ambiguous for both.
    let mut claims: HashMap<usize, usize> = HashMap::new();
    for c in best.values() {
        *claims.entry(*c).or_default() += 1;
    }
    best.retain(|_, c| claims.get(c) == Some(&1));
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(keys: &[u64]) -> Candidate {
        Candidate {
            kind: ModelKind::Folder,
            keys: keys.iter().map(|&k| (k, k)).collect(),
        }
    }

    fn arrival(kind: ModelKind, keys: &[u64]) -> Arrival {
        Arrival {
            kind,
            keys: keys.iter().map(|&k| Some((k, k))).collect(),
        }
    }

    #[test]
    fn exact_folder_match() {
        let m = match_moves(
            &[arrival(ModelKind::Folder, &[1, 2, 3])],
            &[folder(&[9]), folder(&[3, 1, 2])],
        );
        assert_eq!(m.get(&0), Some(&1));
    }

    #[test]
    fn four_of_five_parts_match() {
        let m = match_moves(
            &[arrival(ModelKind::Folder, &[1, 2, 3, 4, 50])],
            &[folder(&[1, 2, 3, 4, 5])],
        );
        assert_eq!(m.get(&0), Some(&0));
    }

    #[test]
    fn three_of_five_is_not_enough() {
        let m = match_moves(
            &[arrival(ModelKind::Folder, &[1, 2, 3, 40, 50])],
            &[folder(&[1, 2, 3, 4, 5])],
        );
        assert!(m.is_empty());
    }

    #[test]
    fn small_model_does_not_swallow_a_big_folder() {
        let m = match_moves(
            &[arrival(ModelKind::Folder, &[1, 2, 3, 4, 5, 6])],
            &[folder(&[1])],
        );
        assert!(m.is_empty());
    }

    #[test]
    fn two_arrivals_for_one_candidate_match_neither() {
        let m = match_moves(
            &[
                arrival(ModelKind::Folder, &[1, 2]),
                arrival(ModelKind::Folder, &[1, 2]),
            ],
            &[folder(&[1, 2])],
        );
        assert!(m.is_empty());
    }

    #[test]
    fn two_equal_candidates_match_neither() {
        let m = match_moves(
            &[arrival(ModelKind::Folder, &[1, 2])],
            &[folder(&[1, 2]), folder(&[1, 2])],
        );
        assert!(m.is_empty());
    }

    #[test]
    fn single_file_needs_same_kind_and_key() {
        let file = Candidate {
            kind: ModelKind::SingleFile,
            keys: vec![(7, 7)],
        };
        let m = match_moves(
            &[
                arrival(ModelKind::Folder, &[7]),
                arrival(ModelKind::SingleFile, &[7]),
            ],
            &[file],
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(&1), Some(&0));
    }

    #[test]
    fn unfingerprinted_arrival_is_skipped() {
        let a = Arrival {
            kind: ModelKind::Folder,
            keys: vec![Some((1, 1)), None],
        };
        assert!(match_moves(&[a], &[folder(&[1])]).is_empty());
    }
}
