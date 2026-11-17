//! What the user knows about a model: notes, source, tags, collections, the
//! recipe and the print log (v1 design §6).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ids::{AttemptId, CollectionId, PhotoId};

/// Longest tag, in characters.
pub const TAG_MAX_CHARS: usize = 64;
/// Longest collection name, in characters.
pub const COLLECTION_MAX_CHARS: usize = 80;
/// Longest free-text field (notes, attempt notes), in bytes.
pub const TEXT_MAX_BYTES: usize = 64 * 1024;

/// A filament material. Unknown names are kept as typed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Material {
    Pla,
    Petg,
    Abs,
    Asa,
    Tpu,
    Pa,
    Pc,
    Other(String),
}

impl Material {
    /// Materials with their own entry in menus, in menu order.
    pub const KNOWN: [Material; 7] = [
        Material::Pla,
        Material::Petg,
        Material::Abs,
        Material::Asa,
        Material::Tpu,
        Material::Pa,
        Material::Pc,
    ];

    /// Parses what a user typed: the first word decides, ignoring case
    /// ("TPU 95A" is TPU, "PLA+" is PLA). Blank is `None`.
    pub fn parse(s: &str) -> Option<Material> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let word: String = s
            .split(|c: char| c.is_whitespace() || c == '-' || c == '+' || c == '/')
            .next()
            .unwrap_or(s)
            .to_uppercase();
        Some(match word.as_str() {
            "PLA" => Material::Pla,
            "PETG" | "PET" => Material::Petg,
            "ABS" => Material::Abs,
            "ASA" => Material::Asa,
            "TPU" | "TPE" => Material::Tpu,
            "PA" | "NYLON" | "PA6" | "PA12" => Material::Pa,
            "PC" => Material::Pc,
            _ => Material::Other(s.to_owned()),
        })
    }

    /// Display and storage name.
    pub fn label(&self) -> &str {
        match self {
            Material::Pla => "PLA",
            Material::Petg => "PETG",
            Material::Abs => "ABS",
            Material::Asa => "ASA",
            Material::Tpu => "TPU",
            Material::Pa => "PA",
            Material::Pc => "PC",
            Material::Other(s) => s,
        }
    }

    /// Lower-case key used in the database for filtering.
    pub fn key(&self) -> String {
        self.label().to_lowercase()
    }
}

impl fmt::Display for Material {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl Serialize for Material {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for Material {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Material::parse(&s).ok_or_else(|| serde::de::Error::custom("empty material"))
    }
}

/// Support structures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Supports {
    None,
    Normal,
    Tree,
}

/// Bed adhesion helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Adhesion {
    None,
    Skirt,
    Brim,
    Raft,
}

/// How a print went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptResult {
    Success,
    Partial,
    Failed,
}

impl AttemptResult {
    pub fn as_str(self) -> &'static str {
        match self {
            AttemptResult::Success => "success",
            AttemptResult::Partial => "partial",
            AttemptResult::Failed => "failed",
        }
    }

    pub fn from_db(s: &str) -> Option<AttemptResult> {
        match s {
            "success" => Some(AttemptResult::Success),
            "partial" => Some(AttemptResult::Partial),
            "failed" => Some(AttemptResult::Failed),
            _ => None,
        }
    }
}

/// Print settings, shared by recipes, per-part overrides and attempts. Every
/// field is optional; `custom` holds anything else as typed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<Material>,
    /// Brand and colour, free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filament: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "short_f32")]
    pub nozzle_mm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "short_f32")]
    pub layer_mm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub infill_pct: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub walls: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports: Option<Supports>,
    /// Where supports go, free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_where: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adhesion: Option<Adhesion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_temp: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_temp: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printer: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub custom: BTreeMap<String, String>,
}

/// Writes an `f32` as the shortest decimal that reads back to it (0.2, not
/// 0.20000000298023224), for formats that widen floats to `f64`.
fn short_f32<S: serde::Serializer>(v: &Option<f32>, s: S) -> std::result::Result<S::Ok, S::Error> {
    match v {
        Some(v) => s.serialize_f64(v.to_string().parse::<f64>().unwrap_or(f64::from(*v))),
        None => s.serialize_none(),
    }
}

/// Allowed range of a numeric setting, inclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub field: &'static str,
    pub min: f64,
    pub max: f64,
}

pub const NOZZLE_MM: Range = Range {
    field: "nozzle_mm",
    min: 0.1,
    max: 2.0,
};
pub const LAYER_MM: Range = Range {
    field: "layer_mm",
    min: 0.02,
    max: 1.0,
};
pub const INFILL_PCT: Range = Range {
    field: "infill_pct",
    min: 0.0,
    max: 100.0,
};
pub const WALLS: Range = Range {
    field: "walls",
    min: 0.0,
    max: 50.0,
};
pub const NOZZLE_TEMP: Range = Range {
    field: "nozzle_temp",
    min: 0.0,
    max: 500.0,
};
pub const BED_TEMP: Range = Range {
    field: "bed_temp",
    min: 0.0,
    max: 150.0,
};
pub const DURATION_MIN: Range = Range {
    field: "duration_min",
    min: 0.0,
    max: 10_000.0,
};

impl Range {
    /// `Ok` when `v` is inside the range (and finite).
    pub fn check(&self, v: f64) -> Result<()> {
        if v.is_finite() && v >= self.min - 1e-9 && v <= self.max + 1e-9 {
            Ok(())
        } else {
            Err(CoreError::SettingOutOfRange {
                field: self.field,
                min: self.min,
                max: self.max,
            })
        }
    }
}

impl PrintSettings {
    /// Checks every numeric field against its range and text lengths.
    pub fn validate(&self) -> Result<()> {
        let checks: [(Range, Option<f64>); 6] = [
            (NOZZLE_MM, self.nozzle_mm.map(f64::from)),
            (LAYER_MM, self.layer_mm.map(f64::from)),
            (INFILL_PCT, self.infill_pct.map(f64::from)),
            (WALLS, self.walls.map(f64::from)),
            (NOZZLE_TEMP, self.nozzle_temp.map(f64::from)),
            (BED_TEMP, self.bed_temp.map(f64::from)),
        ];
        for (range, v) in checks {
            if let Some(v) = v {
                range.check(v)?;
            }
        }
        let texts = [
            &self.filament,
            &self.supports_where,
            &self.orientation,
            &self.printer,
        ];
        if texts
            .iter()
            .any(|t| t.as_ref().is_some_and(|t| t.len() > 1024))
            || self.custom.len() > 100
            || self
                .custom
                .iter()
                .any(|(k, v)| k.len() > 256 || v.len() > 1024)
        {
            return Err(CoreError::TextTooLong);
        }
        Ok(())
    }

    /// Whether nothing is set.
    pub fn is_empty(&self) -> bool {
        *self == PrintSettings::default()
    }

    /// Plain words for full-text search.
    pub fn search_text(&self) -> String {
        let mut out = Vec::new();
        if let Some(m) = &self.material {
            out.push(m.label().to_owned());
        }
        for s in [
            &self.filament,
            &self.supports_where,
            &self.orientation,
            &self.printer,
        ]
        .into_iter()
        .flatten()
        {
            out.push(s.clone());
        }
        for (k, v) in &self.custom {
            out.push(format!("{k} {v}"));
        }
        out.join(" ")
    }
}

/// A recipe note, optionally only for one material ("PETG: warps unless…").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeNote {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<Material>,
    pub text: String,
}

/// Settings that differ for one part (by file name).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartOverride {
    pub part: String,
    #[serde(flatten)]
    pub settings: PrintSettings,
}

/// The distilled "how to print this" for a model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recipe {
    pub settings: PrintSettings,
    /// Setting field names that are required rather than recommended.
    pub required: BTreeSet<String>,
    pub notes: Vec<RecipeNote>,
    pub parts: Vec<PartOverride>,
    pub from_attempt: Option<AttemptId>,
    pub updated_at: i64,
}

impl Recipe {
    pub fn validate(&self) -> Result<()> {
        self.settings.validate()?;
        for p in &self.parts {
            p.settings.validate()?;
        }
        if self.notes.len() > 100 || self.notes.iter().any(|n| n.text.len() > TEXT_MAX_BYTES) {
            return Err(CoreError::TextTooLong);
        }
        Ok(())
    }

    /// Plain words for full-text search.
    pub fn search_text(&self) -> String {
        let mut out = vec![self.settings.search_text()];
        out.extend(self.notes.iter().map(|n| n.text.clone()));
        out.join(" ")
    }
}

/// A photo of an attempt, stored in Printdex's data folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhotoRef {
    pub id: PhotoId,
    pub path: std::path::PathBuf,
}

/// One print attempt.
#[derive(Clone, Debug, PartialEq)]
pub struct Attempt {
    pub id: AttemptId,
    /// When it was printed, unix milliseconds.
    pub printed_at: i64,
    pub result: AttemptResult,
    pub settings: PrintSettings,
    pub duration_min: Option<u32>,
    /// What went wrong, or anything else.
    pub notes: String,
    /// File names of the parts printed; empty means the whole model.
    pub parts: Vec<String>,
    pub photos: Vec<PhotoRef>,
    /// 1 for the oldest attempt of the model.
    pub number: u32,
    pub updated_at: i64,
}

/// A new attempt, as the "Log a print" form sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct NewAttempt {
    pub printed_at: i64,
    pub result: AttemptResult,
    pub settings: PrintSettings,
    pub duration_min: Option<u32>,
    pub notes: String,
    pub parts: Vec<String>,
}

/// A collection with its model count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionInfo {
    pub id: CollectionId,
    pub name: String,
    pub count: u64,
}

/// A tag with its model count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCount {
    pub name: String,
    pub count: u64,
}

/// The most recent attempt of a model, for cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastPrint {
    pub result: AttemptResult,
    pub at: i64,
    /// Successful attempts so far.
    pub successes: u32,
}

/// Everything the user wrote about a model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelData {
    pub notes: String,
    pub source: Option<String>,
    /// Tag names as first typed, sorted ignoring case.
    pub tags: Vec<String>,
    /// Collections the model is in, by name.
    pub collections: Vec<CollectionInfo>,
    pub recipe: Option<Recipe>,
    /// Newest first.
    pub attempts: Vec<Attempt>,
}

impl ModelData {
    /// The material previews are drawn in: the recipe's, else the latest
    /// attempt's (same rule as the thumbnail cache).
    pub fn preview_material(&self) -> printdex_render::Material {
        let m = self
            .recipe
            .as_ref()
            .and_then(|r| r.settings.material.as_ref())
            .or_else(|| {
                self.attempts
                    .first()
                    .and_then(|a| a.settings.material.as_ref())
            });
        crate::db::render_material(m.map(Material::key).as_deref())
    }
}

/// The matching key for a tag or collection name: trimmed, inner whitespace
/// collapsed to one space, lower-cased.
pub fn name_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A tag or collection name as stored: trimmed, inner whitespace collapsed.
pub fn clean_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Accepts `http://` and `https://` links only.
pub fn check_source(url: &str) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    match rest {
        Some(r)
            if !r.is_empty() && url.len() <= 2048 && !url.trim().contains(char::is_whitespace) =>
        {
            Ok(())
        }
        _ => Err(CoreError::SourceInvalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_parse() {
        assert_eq!(Material::parse("petg"), Some(Material::Petg));
        assert_eq!(Material::parse("TPU 95A"), Some(Material::Tpu));
        assert_eq!(Material::parse("PLA+"), Some(Material::Pla));
        assert_eq!(
            Material::parse(" Wood fill "),
            Some(Material::Other("Wood fill".into()))
        );
        assert_eq!(Material::parse("  "), None);
        assert_eq!(Material::Petg.label(), "PETG");
    }

    #[test]
    fn settings_validate_ranges() {
        let ok = PrintSettings {
            bed_temp: Some(85),
            layer_mm: Some(0.2),
            ..PrintSettings::default()
        };
        assert!(ok.validate().is_ok());
        let bad = PrintSettings {
            bed_temp: Some(850),
            ..PrintSettings::default()
        };
        match bad.validate() {
            Err(CoreError::SettingOutOfRange { field, min, max }) => {
                assert_eq!((field, min, max), ("bed_temp", 0.0, 150.0));
            }
            other => panic!("{other:?}"),
        }
        let nan = PrintSettings {
            layer_mm: Some(f32::NAN),
            ..PrintSettings::default()
        };
        assert!(nan.validate().is_err());
    }

    #[test]
    fn settings_json_skips_empty_fields() {
        let s = PrintSettings {
            material: Some(Material::Petg),
            supports: Some(Supports::Tree),
            ..PrintSettings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"material":"PETG","supports":"tree"}"#);
        let back: PrintSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn names_are_normalised() {
        assert_eq!(name_key("  Desk   Organizers "), "desk organizers");
        assert_eq!(clean_name("  Desk   Organizers "), "Desk Organizers");
        assert_eq!(name_key("ÜBER"), "über");
    }

    #[test]
    fn source_must_be_http() {
        assert!(check_source("https://www.printables.com/model/1").is_ok());
        assert!(check_source("http://x.org").is_ok());
        assert!(check_source("file:///etc/passwd").is_err());
        assert!(check_source("javascript:alert(1)").is_err());
        assert!(check_source("https://").is_err());
    }
}
