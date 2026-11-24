//! Merging other installs' records (plan Task 5): per field, the newest
//! stamp wins. Every install applies the same rule to the same records, so
//! all of them end up with the same values whatever order batches arrive
//! in.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::log::{Key, Record, Stamp};
use super::state::value_hash;
use crate::db::FieldRow;

/// What to do with a remote record.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Older than (or the same as) what this install has.
    Ignore,
    /// Newer: take it.
    Apply,
    /// Newer, but it replaces text its writer never saw: take it and keep
    /// the replaced text as "Edited elsewhere".
    ApplyKeepLoser {
        loser: serde_json::Value,
        loser_stamp: Stamp,
    },
}

/// Decides about `rec`, written by `writer` in a batch that had applied
/// `observed` (highest seq per other install).
pub fn decide(
    local: Option<&FieldRow>,
    rec: &Record,
    writer: Uuid,
    observed: &BTreeMap<Uuid, u64>,
) -> Outcome {
    let Some(local) = local else {
        return Outcome::Apply;
    };
    if local.stamp >= rec.stamp {
        return Outcome::Ignore;
    }
    let unseen = local.stamp.install != writer
        && observed.get(&local.stamp.install).copied().unwrap_or(0) < local.seq;
    let text = local.value.as_str().is_some_and(|t| !t.trim().is_empty());
    if rec.key.is_text() && unseen && text && local.value != rec.value {
        return Outcome::ApplyKeepLoser {
            loser: local.value.clone(),
            loser_stamp: local.stamp,
        };
    }
    Outcome::Apply
}

/// The row a taken record leaves.
pub fn row_for(rec: &Record, model: Option<Uuid>, seq: u64) -> FieldRow {
    FieldRow {
        key: rec.key.text(),
        model,
        stamp: rec.stamp,
        seq,
        hash: value_hash(&rec.value),
        value: rec.value.clone(),
    }
}

/// Merges two rows of the same key (when two model ids turn out to be one
/// model): the newer stamp wins.
pub fn newer(a: FieldRow, b: FieldRow) -> FieldRow {
    if b.stamp > a.stamp { b } else { a }
}

/// Who keeps a catalogue number two models claim: the older claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberOutcome {
    /// The model that already has the number keeps it; the incoming one
    /// gets the next free number.
    HolderKeeps,
    /// The incoming model takes the number; the holder gets the next free.
    IncomingTakes,
}

pub fn resolve_number(holder: Option<Stamp>, incoming: Stamp) -> NumberOutcome {
    match holder {
        Some(h) if h <= incoming => NumberOutcome::HolderKeeps,
        _ => NumberOutcome::IncomingTakes,
    }
}

/// Key of a record after following model aliases.
pub fn canonical(key: &Key, winner: Option<Uuid>) -> Key {
    match winner {
        Some(w) => key.with_model(w),
        None => key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::log::{Hlc, ModelField};
    use serde_json::json;
    use std::collections::HashMap;

    const LIB: Uuid = Uuid::from_u128(1);
    const MODEL: Uuid = Uuid::from_u128(2);

    fn st(wall: i64, install: u128) -> Stamp {
        Stamp {
            wall_ms: wall,
            counter: 0,
            install: Uuid::from_u128(install),
        }
    }

    fn notes(v: &str, stamp: Stamp) -> Record {
        Record {
            stamp,
            key: Key::Model {
                lib: LIB,
                model: MODEL,
                field: ModelField::Notes,
            },
            value: json!(v),
        }
    }

    #[test]
    fn older_record_ignored() {
        let local = row_for(&notes("mine", st(200, 1)), Some(MODEL), 1);
        let rec = notes("theirs", st(100, 2));
        assert_eq!(
            decide(Some(&local), &rec, rec.stamp.install, &BTreeMap::new()),
            Outcome::Ignore
        );
    }

    #[test]
    fn equal_stamp_ignored() {
        let r = notes("same", st(100, 2));
        let local = row_for(&r, Some(MODEL), 1);
        assert_eq!(
            decide(Some(&local), &r, r.stamp.install, &BTreeMap::new()),
            Outcome::Ignore
        );
    }

    #[test]
    fn newer_record_applied() {
        let rec = notes("theirs", st(300, 2));
        assert_eq!(
            decide(None, &rec, rec.stamp.install, &BTreeMap::new()),
            Outcome::Apply
        );
        // Not text: plain apply even when concurrent.
        let mut num = rec.clone();
        num.key = Key::Model {
            lib: LIB,
            model: MODEL,
            field: ModelField::Number,
        };
        let local = row_for(
            &Record {
                value: json!(4),
                ..num.clone()
            },
            Some(MODEL),
            5,
        );
        let local = FieldRow {
            stamp: st(100, 1),
            ..local
        };
        assert_eq!(
            decide(Some(&local), &num, num.stamp.install, &BTreeMap::new()),
            Outcome::Apply
        );
    }

    #[test]
    fn concurrent_notes_keep_loser() {
        let local = row_for(&notes("mine", st(100, 1)), Some(MODEL), 4);
        let rec = notes("theirs", st(300, 2));
        // The writer had applied my batches up to 3, not 4.
        let observed = BTreeMap::from([(Uuid::from_u128(1), 3)]);
        assert_eq!(
            decide(Some(&local), &rec, rec.stamp.install, &observed),
            Outcome::ApplyKeepLoser {
                loser: json!("mine"),
                loser_stamp: st(100, 1)
            }
        );
    }

    #[test]
    fn sequential_notes_no_history() {
        let local = row_for(&notes("mine", st(100, 1)), Some(MODEL), 4);
        let rec = notes("theirs", st(300, 2));
        let observed = BTreeMap::from([(Uuid::from_u128(1), 4)]);
        assert_eq!(
            decide(Some(&local), &rec, rec.stamp.install, &observed),
            Outcome::Apply
        );
        // Overwriting its own earlier text is never a conflict.
        let own = row_for(&notes("draft", st(100, 2)), Some(MODEL), 4);
        assert_eq!(
            decide(Some(&own), &rec, rec.stamp.install, &BTreeMap::new()),
            Outcome::Apply
        );
        // Empty text isn't worth keeping.
        let empty = row_for(&notes("", st(100, 1)), Some(MODEL), 4);
        assert_eq!(
            decide(Some(&empty), &rec, rec.stamp.install, &BTreeMap::new()),
            Outcome::Apply
        );
    }

    #[test]
    fn number_conflict_older_keeps() {
        assert_eq!(
            resolve_number(Some(st(100, 1)), st(200, 2)),
            NumberOutcome::HolderKeeps
        );
        assert_eq!(
            resolve_number(Some(st(300, 1)), st(200, 2)),
            NumberOutcome::IncomingTakes
        );
        assert_eq!(
            resolve_number(None, st(200, 2)),
            NumberOutcome::IncomingTakes
        );
    }

    /// A simulated install: its field rows and the batches it wrote.
    struct Sim {
        id: Uuid,
        hlc: Hlc,
        rows: HashMap<String, FieldRow>,
        seq: u64,
        observed: BTreeMap<Uuid, u64>,
    }

    impl Sim {
        fn apply(
            &mut self,
            writer: Uuid,
            seq: u64,
            observed: &BTreeMap<Uuid, u64>,
            recs: &[Record],
        ) {
            for r in recs {
                let local = self.rows.get(&r.key.text());
                if decide(local, r, writer, observed) != Outcome::Ignore {
                    self.rows.insert(r.key.text(), row_for(r, Some(MODEL), seq));
                }
                self.hlc.observe(r.stamp);
            }
            let o = self.observed.entry(writer).or_default();
            *o = (*o).max(seq);
        }
    }

    /// An install whose clock runs a day ahead does not win every later
    /// edit: each install's clock moves past every stamp it has applied, so
    /// an edit made after seeing the other's always wins, on both sides.
    #[test]
    fn clock_a_day_ahead_does_not_win_later_edits() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let sim = |id: u128| Sim {
            id: Uuid::from_u128(id),
            hlc: Hlc::new(Uuid::from_u128(id), None),
            rows: HashMap::new(),
            seq: 0,
            observed: BTreeMap::new(),
        };
        let (mut ahead, mut right) = (sim(1), sim(2));
        // Edits alternate, a minute apart in real time; `ahead` stamps a
        // day later than the real time.
        for minute in 0..6_i64 {
            let ahead_turn = minute % 2 == 0;
            let (from, to) = if ahead_turn {
                (&mut ahead, &mut right)
            } else {
                (&mut right, &mut ahead)
            };
            let now = minute * 60_000 + if ahead_turn { DAY } else { 0 };
            let rec = notes(&format!("edit {minute}"), from.hlc.tick(now));
            from.seq += 1;
            from.rows
                .insert(rec.key.text(), row_for(&rec, Some(MODEL), from.seq));
            let (writer, seq, observed) = (from.id, from.seq, from.observed.clone());
            to.apply(writer, seq, &observed, std::slice::from_ref(&rec));
            for s in [&ahead, &right] {
                assert_eq!(
                    s.rows[&rec.key.text()].value,
                    json!(format!("edit {minute}")),
                    "minute {minute}"
                );
            }
        }
    }

    /// (writer, seq, observed, records)
    type SimBatch = (Uuid, u64, BTreeMap<Uuid, u64>, Vec<Record>);

    #[test]
    fn converges_any_order() {
        let keys: Vec<Key> = (0..5)
            .map(|i| Key::Tag {
                lib: LIB,
                model: MODEL,
                tag: format!("t{i}"),
            })
            .collect();
        for seed in 0..200_u64 {
            let mut rng = fastrand::Rng::with_seed(seed);
            let mut sims: Vec<Sim> = (0..3)
                .map(|i| Sim {
                    id: Uuid::from_u128(100 + i),
                    hlc: Hlc::new(Uuid::from_u128(100 + i), None),
                    rows: HashMap::new(),
                    seq: 0,
                    observed: BTreeMap::new(),
                })
                .collect();
            let mut batches: Vec<SimBatch> = Vec::new();
            for s in &mut sims {
                let skew = rng.i64(-3_600_000..3_600_000);
                for round in 0..4 {
                    let mut recs = Vec::new();
                    for _ in 0..5 {
                        let key = keys[rng.usize(..keys.len())].clone();
                        let now = 1_000_000 + skew + round * 1000 + rng.i64(0..1000);
                        let stamp = s.hlc.tick(now);
                        let value = if rng.bool() {
                            json!(format!("v{}", rng.u32(..100)))
                        } else {
                            serde_json::Value::Null
                        };
                        let r = Record { stamp, key, value };
                        s.rows
                            .insert(r.key.text(), row_for(&r, Some(MODEL), s.seq + 1));
                        recs.push(r);
                    }
                    s.seq += 1;
                    batches.push((s.id, s.seq, s.observed.clone(), recs));
                }
            }
            for s in &mut sims {
                let mut order: Vec<usize> = (0..batches.len()).collect();
                rng.shuffle(&mut order);
                // Random duplicates.
                for _ in 0..3 {
                    order.push(rng.usize(..batches.len()));
                }
                for i in order {
                    let (w, seq, obs, recs) = &batches[i];
                    if *w != s.id {
                        s.apply(*w, *seq, obs, recs);
                    }
                }
            }
            // Same values everywhere, and each is the newest record's.
            let values = |s: &Sim| -> BTreeMap<String, (Stamp, serde_json::Value)> {
                s.rows
                    .iter()
                    .map(|(k, r)| (k.clone(), (r.stamp, r.value.clone())))
                    .collect()
            };
            let first = values(&sims[0]);
            for s in &sims[1..] {
                assert_eq!(values(s), first, "seed {seed}");
            }
            for (k, (stamp, value)) in &first {
                let newest = batches
                    .iter()
                    .flat_map(|b| &b.3)
                    .filter(|r| &r.key.text() == k)
                    .max_by_key(|r| r.stamp)
                    .unwrap();
                assert_eq!(
                    (stamp, value),
                    (&newest.stamp, &newest.value),
                    "seed {seed}"
                );
            }
        }
    }
}
