//! Capturing local changes (plan Task 4): each changed model's current
//! state is compared with what this install last published, and every
//! field that differs becomes a stamped record.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};
use uuid::Uuid;

use super::log::{AttemptField, CollectionField, Hlc, Key, ModelField, RecipeField, Stamp};
use crate::db::FieldRow;
use crate::ids::{CollectionId, PhotoId};
use crate::model_data::{ModelData, name_key};

/// Hash of a value's canonical JSON (object keys are sorted).
pub fn value_hash(v: &Value) -> u64 {
    xxhash_rust::xxh3::xxh3_64(&serde_json::to_vec(v).unwrap_or_default())
}

/// Where a model is and what it holds, as other installs match it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub path: String,
    /// Sorted, distinct part fingerprints.
    pub fps: Vec<u64>,
}

impl Identity {
    pub fn to_value(&self) -> Value {
        json!({ "path": self.path, "fps": self.fps })
    }

    pub fn from_value(v: &Value) -> Option<Identity> {
        let path = v.get("path")?.as_str()?.to_owned();
        let mut fps: Vec<u64> = v
            .get("fps")?
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .collect();
        fps.sort_unstable();
        fps.dedup();
        Some(Identity { path, fps })
    }
}

/// One field of the current state, with the time its entity last changed
/// (used as the stamp the first time the field is published).
#[derive(Clone, Debug, PartialEq)]
pub struct Current {
    pub value: Value,
    pub changed_at: i64,
}

/// Everything a model contributes, by key.
pub type ModelState = BTreeMap<Key, Current>;

/// What a model's data looks like as sync keys.
///
/// `photo` gives the value of a photo key (`{"hash","ext","name"}`), or
/// `None` while the photo file can't be read (it is left out then).
#[allow(clippy::too_many_arguments)]
pub fn model_state(
    lib: Uuid,
    model: Uuid,
    data: &ModelData,
    data_updated_at: i64,
    number: Option<u32>,
    identity: &Identity,
    collections: &HashMap<CollectionId, Uuid>,
    photo: &mut dyn FnMut(PhotoId) -> Option<Value>,
    now: i64,
) -> ModelState {
    let mut s = ModelState::new();
    // Never edited: the oldest possible stamp, so any real edit elsewhere wins.
    let at = data_updated_at.max(0);
    let mut put = |k: Key, value: Value, changed_at: i64| {
        s.insert(k, Current { value, changed_at });
    };
    put(Key::Identity { lib, model }, identity.to_value(), now);
    let field = |f| Key::Model {
        lib,
        model,
        field: f,
    };
    put(field(ModelField::Notes), json!(data.notes), at);
    put(field(ModelField::Source), json!(data.source), at);
    put(field(ModelField::Number), json!(number), at);
    for t in &data.tags {
        put(
            Key::Tag {
                lib,
                model,
                tag: name_key(t),
            },
            json!(t),
            at,
        );
    }
    for c in &data.collections {
        if let Some(&id) = collections.get(&c.id) {
            put(Key::Member { id, lib, model }, json!(true), at);
        }
    }
    let recipe = |f| Key::Recipe {
        lib,
        model,
        field: f,
    };
    match &data.recipe {
        None => put(recipe(RecipeField::Deleted), json!(true), at),
        Some(r) => {
            let rat = if r.updated_at > 0 { r.updated_at } else { at };
            put(recipe(RecipeField::Deleted), json!(false), rat);
            put(recipe(RecipeField::Settings), json!(r.settings), rat);
            put(recipe(RecipeField::Required), json!(r.required), rat);
            put(recipe(RecipeField::Notes), json!(r.notes), rat);
            put(recipe(RecipeField::Parts), json!(r.parts), rat);
            put(
                recipe(RecipeField::FromAttempt),
                json!(r.from_attempt.map(|a| a.0)),
                rat,
            );
        }
    }
    for a in &data.attempts {
        let aat = if a.updated_at > 0 { a.updated_at } else { at };
        let key = |f| Key::Attempt {
            lib,
            model,
            attempt: a.id.0,
            field: f,
        };
        put(key(AttemptField::Deleted), json!(false), aat);
        put(key(AttemptField::PrintedAt), json!(a.printed_at), aat);
        put(key(AttemptField::Result), json!(a.result), aat);
        put(key(AttemptField::Settings), json!(a.settings), aat);
        put(key(AttemptField::DurationMin), json!(a.duration_min), aat);
        put(key(AttemptField::Notes), json!(a.notes), aat);
        put(key(AttemptField::Parts), json!(a.parts), aat);
        for p in &a.photos {
            if let Some(v) = photo(p.id) {
                put(
                    Key::Photo {
                        lib,
                        model,
                        attempt: a.id.0,
                        photo: p.id.0,
                    },
                    v,
                    aat,
                );
            }
        }
    }
    s
}

/// The keys of collections themselves (name, deleted), for every local
/// collection: `(sync id, name)`.
pub fn collection_state(local: &[(Uuid, String)], now: i64) -> ModelState {
    let mut s = ModelState::new();
    for (id, name) in local {
        s.insert(
            Key::Collection {
                id: *id,
                field: CollectionField::Name,
            },
            Current {
                value: json!(name),
                changed_at: now,
            },
        );
        s.insert(
            Key::Collection {
                id: *id,
                field: CollectionField::Deleted,
            },
            Current {
                value: json!(false),
                changed_at: now,
            },
        );
    }
    s
}

/// A change found by [`diff`], not stamped yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub key: Key,
    pub value: Value,
    /// Stamp time for a field published for the first time.
    pub first_at: Option<i64>,
}

/// What changed between the published rows and the current state.
/// Removed tags, memberships and photos become `null`, removed attempts
/// and collections `Deleted = true`; recipe fields of a removed recipe
/// stay as they were (its `Deleted` key says it's gone).
pub fn diff(published: &[FieldRow], current: &ModelState) -> Vec<Change> {
    let by_key: HashMap<&str, &FieldRow> = published.iter().map(|r| (r.key.as_str(), r)).collect();
    let mut out = Vec::new();
    for (key, cur) in current {
        let text = key.text();
        match by_key.get(text.as_str()) {
            Some(row) if row.hash == value_hash(&cur.value) => {}
            Some(_) => out.push(Change {
                key: key.clone(),
                value: cur.value.clone(),
                first_at: None,
            }),
            None => out.push(Change {
                key: key.clone(),
                value: cur.value.clone(),
                first_at: Some(cur.changed_at),
            }),
        }
    }
    for row in published {
        let Some(key) = Key::parse(&row.key) else {
            continue;
        };
        if current.contains_key(&key) {
            continue;
        }
        let gone = match &key {
            Key::Tag { .. } | Key::Member { .. } | Key::Photo { .. } => {
                (!row.value.is_null()).then_some(Value::Null)
            }
            Key::Attempt {
                field: AttemptField::Deleted,
                ..
            }
            | Key::Collection {
                field: CollectionField::Deleted,
                ..
            } => (row.value != json!(true)).then_some(json!(true)),
            _ => None,
        };
        if let Some(value) = gone {
            out.push(Change {
                key,
                value,
                first_at: None,
            });
        }
    }
    out
}

/// Stamps a change: the entity's own change time the first time a field
/// is published (so two machines used apart merge by what's newer), the
/// clock otherwise.
pub fn stamp(change: &Change, hlc: &mut Hlc, now: i64) -> Stamp {
    match change.first_at {
        Some(at) if at >= 0 && at < now => Stamp {
            wall_ms: at,
            counter: 0,
            install: hlc.last().install,
        },
        _ => hlc.tick(now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AttemptId, CollectionId};
    use crate::model_data::{Attempt, AttemptResult, CollectionInfo, PrintSettings, Recipe};

    const LIB: Uuid = Uuid::from_u128(1);
    const MODEL: Uuid = Uuid::from_u128(2);
    const ME: Uuid = Uuid::from_u128(3);

    fn ident() -> Identity {
        Identity {
            path: "dragon".into(),
            fps: vec![1, 2],
        }
    }

    fn state(d: &ModelData) -> ModelState {
        let mut photo = |_| None;
        model_state(
            LIB,
            MODEL,
            d,
            100,
            Some(7),
            &ident(),
            &HashMap::new(),
            &mut photo,
            1000,
        )
    }

    /// Published rows equal to `s` (what a finished capture leaves).
    fn published(s: &ModelState) -> Vec<FieldRow> {
        s.iter()
            .map(|(k, c)| FieldRow {
                key: k.text(),
                model: Some(MODEL),
                stamp: Stamp {
                    wall_ms: 1,
                    counter: 0,
                    install: ME,
                },
                seq: 1,
                hash: value_hash(&c.value),
                value: c.value.clone(),
            })
            .collect()
    }

    fn attempt() -> Attempt {
        Attempt {
            id: AttemptId(Uuid::from_u128(9)),
            printed_at: 50,
            result: AttemptResult::Success,
            settings: PrintSettings::default(),
            duration_min: None,
            notes: String::new(),
            parts: vec![],
            photos: vec![],
            number: 1,
            updated_at: 60,
        }
    }

    #[test]
    fn unchanged_state_no_records() {
        let d = ModelData::default();
        let s = state(&d);
        assert!(diff(&published(&s), &s).is_empty());
    }

    #[test]
    fn first_publish_uses_change_time() {
        let s = state(&ModelData::default());
        let changes = diff(&[], &s);
        let notes = changes
            .iter()
            .find(|c| {
                matches!(
                    c.key,
                    Key::Model {
                        field: ModelField::Notes,
                        ..
                    }
                )
            })
            .unwrap();
        assert_eq!(notes.first_at, Some(100));
        let mut hlc = Hlc::new(ME, None);
        assert_eq!(stamp(notes, &mut hlc, 1000).wall_ms, 100);
    }

    #[test]
    fn notes_edit_one_record() {
        let before = state(&ModelData::default());
        let after = state(&ModelData {
            notes: "Print slow.".into(),
            ..ModelData::default()
        });
        let changes = diff(&published(&before), &after);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].value, json!("Print slow."));
        assert_eq!(changes[0].first_at, None);
    }

    #[test]
    fn removed_tag_tombstone() {
        let before = state(&ModelData {
            tags: vec!["Dragon".into()],
            ..ModelData::default()
        });
        let after = state(&ModelData::default());
        let changes = diff(&published(&before), &after);
        assert_eq!(changes.len(), 1);
        assert!(matches!(&changes[0].key, Key::Tag { tag, .. } if tag == "dragon"));
        assert!(changes[0].value.is_null());
        // A tombstone already published is not sent again.
        let mut rows = published(&before);
        rows[0..].iter_mut().for_each(|r| {
            if r.key.contains("\"tag\"") {
                r.value = Value::Null;
                r.hash = value_hash(&Value::Null);
            }
        });
        assert!(diff(&rows, &after).is_empty());
    }

    #[test]
    fn deleted_attempt_tombstone() {
        let before = state(&ModelData {
            attempts: vec![attempt()],
            ..ModelData::default()
        });
        let after = state(&ModelData::default());
        let changes = diff(&published(&before), &after);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes[0].key,
            Key::Attempt {
                field: AttemptField::Deleted,
                ..
            }
        ));
        assert_eq!(changes[0].value, json!(true));
    }

    #[test]
    fn recipe_removed_marks_deleted() {
        let before = state(&ModelData {
            recipe: Some(Recipe {
                notes: vec![],
                ..Recipe::default()
            }),
            ..ModelData::default()
        });
        let after = state(&ModelData::default());
        let changes = diff(&published(&before), &after);
        assert_eq!(changes.len(), 1);
        assert!(matches!(
            changes[0].key,
            Key::Recipe {
                field: RecipeField::Deleted,
                ..
            }
        ));
        assert_eq!(changes[0].value, json!(true));
    }

    #[test]
    fn collection_membership_uses_sync_id() {
        let cid = CollectionId(Uuid::from_u128(40));
        let sync = Uuid::from_u128(41);
        let d = ModelData {
            collections: vec![CollectionInfo {
                id: cid,
                name: "Gifts".into(),
                count: 1,
            }],
            ..ModelData::default()
        };
        let mut photo = |_| None;
        let s = model_state(
            LIB,
            MODEL,
            &d,
            100,
            None,
            &ident(),
            &HashMap::from([(cid, sync)]),
            &mut photo,
            1000,
        );
        assert!(s.contains_key(&Key::Member {
            id: sync,
            lib: LIB,
            model: MODEL
        }));
    }

    #[test]
    fn identity_value_roundtrip() {
        let i = ident();
        assert_eq!(Identity::from_value(&i.to_value()), Some(i));
    }
}
