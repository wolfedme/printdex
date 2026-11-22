//! Change records, the hybrid logical clock that orders them, and the
//! batch files they travel in (design §6).
//!
//! A batch file is `PDXS`, a version byte, a flags byte (bit 0: sealed),
//! then zstd-compressed JSON lines: a header, one record per line and a
//! trailer with the record count. A file without its trailer was cut
//! short and is never applied.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::crypto::{self, SpaceKey};
use crate::error::SyncError;

/// Format of batches, snapshots and `space.toml` this build writes.
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"PDXS";
const FLAG_SEALED: u8 = 1;

/// When a change was made, totally ordered across installs: wall clock,
/// then a counter for changes in the same millisecond, then the install.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Stamp {
    #[serde(rename = "w")]
    pub wall_ms: i64,
    #[serde(rename = "c")]
    pub counter: u32,
    #[serde(rename = "i")]
    pub install: Uuid,
}

/// A hybrid logical clock: never goes back, even when the wall clock does,
/// and moves past every stamp it has seen.
#[derive(Clone, Debug)]
pub struct Hlc {
    last: Stamp,
}

impl Hlc {
    pub fn new(install: Uuid, last: Option<Stamp>) -> Hlc {
        Hlc {
            last: Stamp {
                install,
                ..last.unwrap_or(Stamp {
                    wall_ms: 0,
                    counter: 0,
                    install,
                })
            },
        }
    }

    /// A stamp for a change made now.
    pub fn tick(&mut self, now_ms: i64) -> Stamp {
        if now_ms > self.last.wall_ms {
            self.last.wall_ms = now_ms;
            self.last.counter = 0;
        } else {
            self.last.counter = self.last.counter.saturating_add(1);
        }
        self.last
    }

    /// Moves the clock past a stamp from another install.
    pub fn observe(&mut self, remote: Stamp) {
        if (remote.wall_ms, remote.counter) > (self.last.wall_ms, self.last.counter) {
            self.last.wall_ms = remote.wall_ms;
            self.last.counter = remote.counter;
        }
    }

    /// The last stamp handed out or observed (saved between sessions).
    pub fn last(&self) -> Stamp {
        self.last
    }
}

/// Fields of a model's own data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelField {
    Notes,
    Source,
    Number,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionField {
    Name,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeField {
    Settings,
    Required,
    Notes,
    Parts,
    FromAttempt,
    /// `true` when the model has no recipe.
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptField {
    PrintedAt,
    Result,
    Settings,
    DurationMin,
    Notes,
    Parts,
    Deleted,
}

/// What a record changes. Model keys carry the library's and the model's
/// sync ids.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// Where the model is and what it holds: `{"path", "fps"}`.
    Identity { lib: Uuid, model: Uuid },
    Model {
        lib: Uuid,
        model: Uuid,
        field: ModelField,
    },
    /// Value: the tag as typed, or `null` once removed.
    Tag { lib: Uuid, model: Uuid, tag: String },
    /// A collection (all libraries share them).
    Collection { id: Uuid, field: CollectionField },
    /// Value: `true`, or `null` once removed.
    Member { id: Uuid, lib: Uuid, model: Uuid },
    Recipe {
        lib: Uuid,
        model: Uuid,
        field: RecipeField,
    },
    Attempt {
        lib: Uuid,
        model: Uuid,
        attempt: Uuid,
        field: AttemptField,
    },
    /// Value: `{"hash", "ext", "name"}`, or `null` once removed.
    Photo {
        lib: Uuid,
        model: Uuid,
        attempt: Uuid,
        photo: Uuid,
    },
}

impl Key {
    /// The canonical text stored in `sync_fields.key`.
    pub fn text(&self) -> String {
        serde_json::to_string(self).expect("keys serialize")
    }

    pub fn parse(text: &str) -> Option<Key> {
        serde_json::from_str(text).ok()
    }

    /// The model the key belongs to, if any.
    pub fn model(&self) -> Option<(Uuid, Uuid)> {
        match *self {
            Key::Identity { lib, model }
            | Key::Model { lib, model, .. }
            | Key::Tag { lib, model, .. }
            | Key::Member { lib, model, .. }
            | Key::Recipe { lib, model, .. }
            | Key::Attempt { lib, model, .. }
            | Key::Photo { lib, model, .. } => Some((lib, model)),
            Key::Collection { .. } => None,
        }
    }

    /// The same key for another model sync id (after two ids merged).
    pub fn with_model(&self, to: Uuid) -> Key {
        let mut k = self.clone();
        match &mut k {
            Key::Identity { model, .. }
            | Key::Model { model, .. }
            | Key::Tag { model, .. }
            | Key::Member { model, .. }
            | Key::Recipe { model, .. }
            | Key::Attempt { model, .. }
            | Key::Photo { model, .. } => *model = to,
            Key::Collection { .. } => {}
        }
        k
    }

    /// Text that two machines may have edited at once; the losing side is
    /// kept as "Edited elsewhere".
    pub fn is_text(&self) -> bool {
        matches!(
            self,
            Key::Model {
                field: ModelField::Notes,
                ..
            } | Key::Recipe {
                field: RecipeField::Notes,
                ..
            } | Key::Attempt {
                field: AttemptField::Notes,
                ..
            }
        )
    }
}

/// One field-level change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(rename = "s")]
    pub stamp: Stamp,
    #[serde(rename = "k")]
    pub key: Key,
    #[serde(rename = "v")]
    pub value: serde_json::Value,
}

/// A numbered file of records from one install.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub format: u32,
    pub install: Uuid,
    pub seq: u64,
    /// For each other install, the highest batch seq the writer had applied
    /// when it wrote this one.
    #[serde(default)]
    pub observed: BTreeMap<Uuid, u64>,
    #[serde(skip)]
    pub records: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
struct Trailer {
    end: usize,
}

/// Writes a batch (or snapshot) file.
pub fn encode(batch: &Batch, key: Option<&SpaceKey>) -> Vec<u8> {
    let mut lines = Vec::new();
    let mut push = |v: &dyn erased::Line| {
        lines.extend_from_slice(&v.json());
        lines.push(b'\n');
    };
    push(batch);
    for r in &batch.records {
        push(r);
    }
    push(&Trailer {
        end: batch.records.len(),
    });
    let packed = zstd::bulk::compress(&lines, 3).expect("zstd compression of a buffer");
    let mut out = Vec::with_capacity(packed.len() + 48);
    out.extend_from_slice(MAGIC);
    out.push(u8::try_from(FORMAT).unwrap_or(u8::MAX));
    match key {
        Some(k) => {
            out.push(FLAG_SEALED);
            out.extend_from_slice(&crypto::seal(k, &packed));
        }
        None => {
            out.push(0);
            out.extend_from_slice(&packed);
        }
    }
    out
}

mod erased {
    pub trait Line {
        fn json(&self) -> Vec<u8>;
    }
    impl<T: serde::Serialize> Line for T {
        fn json(&self) -> Vec<u8> {
            serde_json::to_vec(self).expect("sync lines serialize")
        }
    }
}

/// Largest decompressed batch accepted (a snapshot of a very large library
/// stays far below).
const MAX_PLAIN: usize = 512 * 1024 * 1024;

/// Reads a batch (or snapshot) file.
pub fn decode(bytes: &[u8], key: Option<&SpaceKey>) -> Result<Batch, SyncError> {
    if bytes.len() < 6 {
        return Err(SyncError::Truncated);
    }
    if &bytes[..4] != MAGIC {
        return Err(SyncError::Corrupt);
    }
    let version = u32::from(bytes[4]);
    if version > FORMAT {
        return Err(SyncError::NewerFormat(version));
    }
    let body = &bytes[6..];
    let packed = if bytes[5] & FLAG_SEALED != 0 {
        let k = key.ok_or(SyncError::PassphraseNeeded)?;
        crypto::open(k, body)?
    } else {
        body.to_vec()
    };
    let plain = decompress(&packed)?;
    let mut lines = plain.split(|b| *b == b'\n').filter(|l| !l.is_empty());
    let mut batch: Batch = lines
        .next()
        .and_then(|l| serde_json::from_slice(l).ok())
        .ok_or(SyncError::Truncated)?;
    if batch.format > FORMAT {
        return Err(SyncError::NewerFormat(batch.format));
    }
    let mut ended = None;
    for line in lines {
        if ended.is_some() {
            return Err(SyncError::Corrupt);
        }
        if let Ok(r) = serde_json::from_slice::<Record>(line) {
            batch.records.push(r);
        } else if let Ok(t) = serde_json::from_slice::<Trailer>(line) {
            ended = Some(t.end);
        } else {
            // A record this build doesn't know (newer key kinds) is skipped.
            tracing::debug!("skipping an unknown sync record");
        }
    }
    match ended {
        None => Err(SyncError::Truncated),
        Some(_) => Ok(batch),
    }
}

fn decompress(packed: &[u8]) -> Result<Vec<u8>, SyncError> {
    use std::io::Read;
    let mut out = Vec::new();
    let dec = zstd::stream::read::Decoder::new(packed).map_err(|_| SyncError::Truncated)?;
    dec.take(MAX_PLAIN as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| SyncError::Truncated)?;
    if out.len() > MAX_PLAIN {
        return Err(SyncError::Corrupt);
    }
    Ok(out)
}

/// `installs/<install>/log/<seq>.chg`
pub fn batch_path(install: Uuid, seq: u64) -> String {
    format!("installs/{}/log/{seq:020}.chg", install.hyphenated())
}

/// `installs/<install>/snap/<seq>.snap`
pub fn snap_path(install: Uuid, seq: u64) -> String {
    format!("installs/{}/snap/{seq:020}.snap", install.hyphenated())
}

/// `installs/<install>/device.toml`
pub fn device_path(install: Uuid) -> String {
    format!("installs/{}/device.toml", install.hyphenated())
}

pub fn log_dir(install: Uuid) -> String {
    format!("installs/{}/log", install.hyphenated())
}

pub fn snap_dir(install: Uuid) -> String {
    format!("installs/{}/snap", install.hyphenated())
}

/// `photos/<hash>.<ext>`
pub fn photo_path(hash: &str, ext: &str) -> String {
    format!("photos/{hash}.{ext}")
}

/// The seq in a batch or snapshot file name (`<20 digits>.<ext>`); `None`
/// for anything else, such as a temporary file.
pub fn parse_seq(name: &str, ext: &str) -> Option<u64> {
    let digits = name.strip_suffix(ext)?.strip_suffix('.')?;
    if digits.len() != 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::{KdfParams, derive};

    fn me() -> Uuid {
        Uuid::from_u128(1)
    }

    fn sample() -> Batch {
        let mut hlc = Hlc::new(me(), None);
        let lib = Uuid::from_u128(10);
        let model = Uuid::from_u128(20);
        Batch {
            format: FORMAT,
            install: me(),
            seq: 3,
            observed: BTreeMap::from([(Uuid::from_u128(2), 7)]),
            records: vec![
                Record {
                    stamp: hlc.tick(1000),
                    key: Key::Model {
                        lib,
                        model,
                        field: ModelField::Notes,
                    },
                    value: serde_json::json!("Print slow."),
                },
                Record {
                    stamp: hlc.tick(1000),
                    key: Key::Tag {
                        lib,
                        model,
                        tag: "dragon".into(),
                    },
                    value: serde_json::Value::Null,
                },
            ],
        }
    }

    #[test]
    fn hlc_ticks_monotonic() {
        let mut h = Hlc::new(me(), None);
        let a = h.tick(1000);
        let b = h.tick(1000);
        let c = h.tick(1000);
        assert_eq!((a.wall_ms, a.counter), (1000, 0));
        assert_eq!((b.counter, c.counter), (1, 2));
        assert!(a < b && b < c);
    }

    #[test]
    fn hlc_never_goes_back() {
        let mut h = Hlc::new(me(), None);
        let a = h.tick(1000);
        let b = h.tick(500);
        assert_eq!(b.wall_ms, 1000);
        assert!(b > a);
    }

    #[test]
    fn hlc_observe_jumps_ahead() {
        let mut h = Hlc::new(me(), None);
        let remote = Stamp {
            wall_ms: 5000,
            counter: 7,
            install: Uuid::from_u128(9),
        };
        h.observe(remote);
        let next = h.tick(1000);
        assert!(next > remote);
        assert_eq!(next.install, me());
    }

    #[test]
    fn batch_roundtrip_plain() {
        let b = sample();
        assert_eq!(decode(&encode(&b, None), None).unwrap(), b);
    }

    #[test]
    fn batch_roundtrip_encrypted() {
        let k = derive(
            "pw",
            &KdfParams {
                log_n: 4,
                ..KdfParams::generate()
            },
        )
        .unwrap();
        let b = sample();
        let bytes = encode(&b, Some(&k));
        assert_eq!(decode(&bytes, Some(&k)).unwrap(), b);
        assert_eq!(decode(&bytes, None), Err(SyncError::PassphraseNeeded));
    }

    #[test]
    fn wrong_key_fails() {
        let p = KdfParams {
            log_n: 4,
            ..KdfParams::generate()
        };
        let k = derive("pw", &p).unwrap();
        let other = derive("nope", &p).unwrap();
        let bytes = encode(&sample(), Some(&k));
        assert_eq!(
            decode(&bytes, Some(&other)),
            Err(SyncError::WrongPassphrase)
        );
    }

    #[test]
    fn truncated_batch_rejected() {
        let bytes = encode(&sample(), None);
        assert_eq!(
            decode(&bytes[..bytes.len() - 10], None),
            Err(SyncError::Truncated)
        );
        // A complete zstd frame without the trailer line.
        let b = sample();
        let mut lines = serde_json::to_vec(&b).unwrap();
        lines.push(b'\n');
        lines.extend(serde_json::to_vec(&b.records[0]).unwrap());
        let mut f = MAGIC.to_vec();
        f.extend([1, 0]);
        f.extend(zstd::bulk::compress(&lines, 3).unwrap());
        assert_eq!(decode(&f, None), Err(SyncError::Truncated));
        assert_eq!(decode(b"PD", None), Err(SyncError::Truncated));
        assert_eq!(decode(b"NOPE00000", None), Err(SyncError::Corrupt));
    }

    #[test]
    fn newer_format_rejected() {
        let mut b = sample();
        b.format = 2;
        assert_eq!(
            decode(&encode(&b, None), None),
            Err(SyncError::NewerFormat(2))
        );
        let mut bytes = encode(&sample(), None);
        bytes[4] = 9;
        assert_eq!(decode(&bytes, None), Err(SyncError::NewerFormat(9)));
    }

    #[test]
    fn parse_seq_ignores_tmp() {
        assert_eq!(parse_seq("00000000000000000003.chg", "chg"), Some(3));
        assert_eq!(parse_seq(".00000000000000000003.chg.tmp-x", "chg"), None);
        assert_eq!(parse_seq("3.chg", "chg"), None);
        assert_eq!(parse_seq("00000000000000000003.snap", "chg"), None);
        let id = Uuid::from_u128(5);
        assert!(batch_path(id, 3).ends_with("/log/00000000000000000003.chg"));
    }

    #[test]
    fn key_text_roundtrip() {
        let k = Key::Attempt {
            lib: Uuid::from_u128(1),
            model: Uuid::from_u128(2),
            attempt: Uuid::from_u128(3),
            field: AttemptField::Notes,
        };
        assert_eq!(Key::parse(&k.text()), Some(k.clone()));
        assert!(k.is_text());
        assert_eq!(
            k.with_model(Uuid::from_u128(9)).model().unwrap().1,
            Uuid::from_u128(9)
        );
    }
}
