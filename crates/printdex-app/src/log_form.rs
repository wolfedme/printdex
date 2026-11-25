//! The "Log a print" form and the recipe editor: text fields as typed,
//! turned into engine values with messages that name the problem.

use std::collections::BTreeSet;
use std::path::PathBuf;

use iced::widget::text_editor;
use printdex_core::model_data::{self, Range};
use printdex_core::{
    Adhesion, Attempt, AttemptId, AttemptResult, CoreError, Material, NewAttempt, PartOverride,
    PrintSettings, Recipe, RecipeNote, Supports,
};

use crate::i18n::{Arg, t, t_args};

/// What the form edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormMode {
    /// A new attempt.
    Log,
    /// An existing attempt.
    Edit(AttemptId),
    /// The model's recipe.
    Recipe,
}

/// A field that can be wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Date,
    Layer,
    Nozzle,
    Bed,
    Hotend,
    Infill,
    Walls,
    Duration,
    Outcome,
    Text,
}

/// Fields that can be marked "required" in a recipe, with their stored
/// names.
pub const REQUIRABLE: [(&str, &str); 6] = [
    ("material", "form-material"),
    ("nozzle_mm", "form-nozzle"),
    ("layer_mm", "form-layer"),
    ("bed_temp", "form-bed"),
    ("nozzle_temp", "form-hotend"),
    ("supports", "form-supports"),
];

#[derive(Debug)]
pub struct LogForm {
    pub mode: FormMode,
    /// How the print went; a new log starts with none picked so the
    /// user never saves a success by accident.
    pub outcome: Option<AttemptResult>,
    pub printer: String,
    pub date: String,
    pub material: String,
    pub filament: String,
    pub layer: String,
    pub nozzle: String,
    pub bed: String,
    pub hotend: String,
    pub infill: String,
    pub walls: String,
    pub supports: Option<Supports>,
    pub adhesion: Option<Adhesion>,
    pub duration: String,
    pub notes: text_editor::Content,
    /// Images to attach when saving.
    pub photos: Vec<PathBuf>,
    /// Settings came from the recipe.
    pub prefilled: bool,
    /// A prefilled log shows its settings folded into one summary line
    /// until the user opens them to change something.
    pub settings_open: bool,
    /// Recipe editor: required field names and the notes.
    pub required: BTreeSet<String>,
    pub recipe_notes: Vec<RecipeNote>,
    pub new_note: String,
    /// Recipe editor: part file names that can get their own settings.
    pub parts: Vec<String>,
    /// Recipe editor: the part whose settings the fields show, or `None`
    /// for the whole model.
    pub scope: Option<String>,
    /// Whole-model settings, up to date while a part is shown.
    model_settings: PrintSettings,
    /// Per-part settings, up to date except for the part being shown.
    overrides: Vec<PartOverride>,
    /// Settings the form does not show, kept as they were.
    kept: PrintSettings,
    kept_recipe: Recipe,
    pub error: Option<(Field, String)>,
}

/// `0.2` → `"0.20"` for layer heights, `"0.4"` for nozzles: short and exact.
fn num(v: Option<f32>, decimals: usize) -> String {
    v.map(|v| format!("{v:.decimals$}")).unwrap_or_default()
}

fn int<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

/// "2h 41m", "161", "2:41", "2 h", "45m" → minutes.
pub fn parse_duration(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase().replace(' ', "");
    if s.is_empty() {
        return None;
    }
    if let Ok(m) = s.parse::<u32>() {
        return Some(m);
    }
    if let Some((h, m)) = s.split_once(':') {
        return Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?);
    }
    let (h, rest) = match s.split_once('h') {
        Some((h, rest)) => (h.parse::<u32>().ok()?, rest),
        None => (0, s.as_str()),
    };
    let m = match rest.trim_end_matches("min").trim_end_matches('m') {
        "" => 0,
        m => m.parse::<u32>().ok()?,
    };
    Some(h * 60 + m)
}

/// Minutes → "2 h 41 m".
pub fn format_duration(min: u32) -> String {
    match (min / 60, min % 60) {
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} m"),
    }
}

/// Today in the OS time zone as "2026-11-17".
fn today() -> String {
    jiff::ZonedDateTime::now().date().to_string()
}

/// "2026-11-17" → unix ms at noon local time; today → now.
pub fn parse_date(s: &str) -> Option<i64> {
    let date: jiff::civil::Date = s.trim().parse().ok()?;
    let now = jiff::ZonedDateTime::now();
    if date == now.date() {
        return Some(now.timestamp().as_millisecond());
    }
    if date > now.date() {
        return None;
    }
    let zoned = date
        .at(12, 0, 0, 0)
        .to_zoned_datetime(jiff::tz::TimeZone::system())
        .ok()?;
    Some(zoned.timestamp().as_millisecond())
}

/// Unix ms → "2026-11-17" in the OS time zone.
pub fn date_text(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|ts| {
            ts.to_zoned_datetime(jiff::tz::TimeZone::system())
                .date()
                .to_string()
        })
        .unwrap_or_else(|_| today())
}

/// The translated field name for a range.
fn field_label(range: &str) -> (Field, String) {
    match range {
        "nozzle_mm" => (Field::Nozzle, t("form-nozzle")),
        "layer_mm" => (Field::Layer, t("form-layer")),
        "infill_pct" => (Field::Infill, t("form-infill")),
        "walls" => (Field::Walls, t("form-walls")),
        "nozzle_temp" => (Field::Hotend, t("form-hotend")),
        "bed_temp" => (Field::Bed, t("form-bed")),
        "duration_min" => (Field::Duration, t("form-duration")),
        _ => (Field::Text, String::new()),
    }
}

/// "Bed temperature must be between 0 and 150 °C."
pub fn range_message(field: &str, min: f64, max: f64) -> (Field, String) {
    let (f, label) = field_label(field);
    let unit = match field {
        "nozzle_mm" | "layer_mm" => " mm",
        "nozzle_temp" | "bed_temp" => " °C",
        "infill_pct" => " %",
        "duration_min" => " min",
        _ => "",
    };
    (
        f,
        t_args(
            "form-error-range",
            &[
                ("field", Arg::str(label)),
                ("min", Arg::str(format!("{min}{unit}"))),
                ("max", Arg::str(format!("{max}{unit}"))),
            ],
        ),
    )
}

/// The message for an engine error about the form, if it is one.
pub fn error_message(e: &CoreError) -> Option<(Field, String)> {
    match e {
        CoreError::SettingOutOfRange { field, min, max } => Some(range_message(field, *min, *max)),
        CoreError::TextTooLong => Some((Field::Text, t("error-text-too-long"))),
        CoreError::PhotoInvalid => Some((Field::Text, t("form-error-photo"))),
        _ => None,
    }
}

fn parse_f32(s: &str, field: Field, range: Range) -> Result<Option<f32>, (Field, String)> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return Ok(None);
    }
    let v: f64 = s.parse().map_err(|_| {
        let (_, label) = field_label(range.field);
        (
            field,
            t_args("form-error-number", &[("field", Arg::str(label))]),
        )
    })?;
    range
        .check(v)
        .map_err(|_| range_message(range.field, range.min, range.max))?;
    #[allow(clippy::cast_possible_truncation)]
    Ok(Some(v as f32))
}

fn parse_int<T: TryFrom<i64>>(
    s: &str,
    field: Field,
    range: Range,
) -> Result<Option<T>, (Field, String)> {
    let Some(v) = parse_f32(s, field, range)? else {
        return Ok(None);
    };
    #[allow(clippy::cast_possible_truncation)]
    let rounded = f64::from(v).round() as i64;
    Ok(T::try_from(rounded).ok())
}

fn opt(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

impl LogForm {
    fn blank(mode: FormMode) -> LogForm {
        LogForm {
            mode,
            outcome: None,
            printer: String::new(),
            date: today(),
            material: String::new(),
            filament: String::new(),
            layer: String::new(),
            nozzle: String::new(),
            bed: String::new(),
            hotend: String::new(),
            infill: String::new(),
            walls: String::new(),
            supports: None,
            adhesion: None,
            duration: String::new(),
            notes: text_editor::Content::new(),
            photos: Vec::new(),
            prefilled: false,
            settings_open: false,
            required: BTreeSet::new(),
            recipe_notes: Vec::new(),
            new_note: String::new(),
            parts: Vec::new(),
            scope: None,
            model_settings: PrintSettings::default(),
            overrides: Vec::new(),
            kept: PrintSettings::default(),
            kept_recipe: Recipe::default(),
            error: None,
        }
    }

    fn fill(&mut self, s: &PrintSettings) {
        self.material = s
            .material
            .as_ref()
            .map(|m| m.label().to_owned())
            .unwrap_or_default();
        self.filament = s.filament.clone().unwrap_or_default();
        self.layer = num(s.layer_mm, 2);
        self.nozzle = num(s.nozzle_mm, 1);
        self.bed = int(s.bed_temp);
        self.hotend = int(s.nozzle_temp);
        self.infill = int(s.infill_pct);
        self.walls = int(s.walls);
        self.supports = s.supports;
        self.adhesion = s.adhesion;
        self.kept = s.clone();
    }

    /// A new attempt, settings prefilled from the recipe and the printer
    /// from the last attempt.
    pub fn log(recipe: Option<&Recipe>, last_printer: Option<&str>) -> LogForm {
        let mut f = LogForm::blank(FormMode::Log);
        if let Some(r) = recipe {
            f.fill(&r.settings);
            f.prefilled = !r.settings.is_empty();
        }
        if f.printer.is_empty() {
            f.printer = recipe
                .and_then(|r| r.settings.printer.clone())
                .or_else(|| last_printer.map(str::to_owned))
                .unwrap_or_default();
        }
        f
    }

    /// Editing an attempt.
    pub fn edit(a: &Attempt) -> LogForm {
        let mut f = LogForm::blank(FormMode::Edit(a.id));
        f.fill(&a.settings);
        f.outcome = Some(a.result);
        f.printer = a.settings.printer.clone().unwrap_or_default();
        f.date = date_text(a.printed_at);
        f.duration = a.duration_min.map(format_duration).unwrap_or_default();
        f.notes = text_editor::Content::with_text(&a.notes);
        f
    }

    /// Editing the recipe of a model made of `parts` (file names).
    pub fn recipe(r: Option<&Recipe>, parts: &[String]) -> LogForm {
        let mut f = LogForm::blank(FormMode::Recipe);
        if let Some(r) = r {
            f.fill(&r.settings);
            f.printer = r.settings.printer.clone().unwrap_or_default();
            f.required = r.required.clone();
            f.recipe_notes = r.notes.clone();
            f.model_settings = r.settings.clone();
            f.overrides = r.parts.clone();
            f.kept_recipe = r.clone();
        }
        let mut names: Vec<String> = Vec::new();
        // Parts with settings whose file is gone still show, so their
        // settings can be read and removed.
        let saved = f.overrides.iter().map(|o| o.part.as_str());
        for name in parts.iter().map(String::as_str).chain(saved) {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        f.parts = names;
        f
    }

    /// Whether the recipe editor offers per-part settings: the model has
    /// several parts, or a part already has settings.
    pub fn has_part_choice(&self) -> bool {
        self.parts.len() > 1 || !self.overrides.is_empty()
    }

    /// Whether `part` has settings of its own (as typed so far).
    pub fn part_has_settings(&self, part: &str) -> bool {
        if self.scope.as_deref() == Some(part) {
            return self.settings().map_or(true, |s| !s.is_empty());
        }
        self.overrides.iter().any(|o| o.part == part)
    }

    /// The whole-model settings and the per-part settings, with the
    /// fields as typed.
    fn current(&self) -> Result<(PrintSettings, Vec<PartOverride>), (Field, String)> {
        let typed = self.settings()?;
        let Some(part) = &self.scope else {
            return Ok((typed, self.overrides.clone()));
        };
        let mut overrides = self.overrides.clone();
        match overrides.iter().position(|o| &o.part == part) {
            Some(i) if typed.is_empty() => {
                overrides.remove(i);
            }
            Some(i) => {
                if let Some(o) = overrides.get_mut(i) {
                    o.settings = typed;
                }
            }
            None if typed.is_empty() => {}
            None => overrides.push(PartOverride {
                part: part.clone(),
                settings: typed,
            }),
        }
        Ok((self.model_settings.clone(), overrides))
    }

    /// Shows the settings of `part` (`None`: the whole model). Fails, and
    /// stays, when what is typed is not valid.
    pub fn select_scope(&mut self, scope: Option<String>) -> Result<(), (Field, String)> {
        if scope == self.scope {
            return Ok(());
        }
        let (model, overrides) = self.current()?;
        self.model_settings = model;
        self.overrides = overrides;
        let shown = match &scope {
            None => self.model_settings.clone(),
            Some(part) => self
                .overrides
                .iter()
                .find(|o| &o.part == part)
                .map(|o| o.settings.clone())
                .unwrap_or_default(),
        };
        self.scope = scope;
        self.fill(&shown);
        self.printer = shown.printer.unwrap_or_default();
        self.error = None;
        Ok(())
    }

    /// Empties the fields of the part being shown, so it prints like the
    /// rest of the model.
    pub fn clear_part(&mut self) {
        if self.scope.is_some() {
            self.fill(&PrintSettings::default());
            self.printer.clear();
            self.error = None;
        }
    }

    /// What an empty field means while a part is shown: the whole model's
    /// value. Empty for the whole model.
    pub fn hint(&self, input: crate::prints::Input) -> String {
        use crate::prints::Input;
        if self.scope.is_none() {
            return String::new();
        }
        let s = &self.model_settings;
        match input {
            Input::Printer => s.printer.clone().unwrap_or_default(),
            Input::Material => s
                .material
                .as_ref()
                .map(|m| m.label().to_owned())
                .unwrap_or_default(),
            Input::Filament => s.filament.clone().unwrap_or_default(),
            Input::Layer => num(s.layer_mm, 2),
            Input::Nozzle => num(s.nozzle_mm, 1),
            Input::Bed => int(s.bed_temp),
            Input::Hotend => int(s.nozzle_temp),
            Input::Infill => int(s.infill_pct),
            Input::Walls => int(s.walls),
            Input::Date | Input::Duration | Input::NewNote => String::new(),
        }
    }

    /// The settings typed in, checked against their ranges.
    pub fn settings(&self) -> Result<PrintSettings, (Field, String)> {
        let s = PrintSettings {
            material: Material::parse(&self.material),
            filament: opt(&self.filament),
            nozzle_mm: parse_f32(&self.nozzle, Field::Nozzle, model_data::NOZZLE_MM)?,
            layer_mm: parse_f32(&self.layer, Field::Layer, model_data::LAYER_MM)?,
            infill_pct: parse_int(&self.infill, Field::Infill, model_data::INFILL_PCT)?,
            walls: parse_int(&self.walls, Field::Walls, model_data::WALLS)?,
            supports: self.supports,
            adhesion: self.adhesion,
            nozzle_temp: parse_int(&self.hotend, Field::Hotend, model_data::NOZZLE_TEMP)?,
            bed_temp: parse_int(&self.bed, Field::Bed, model_data::BED_TEMP)?,
            printer: opt(&self.printer),
            // Fields the form does not show stay as they were.
            supports_where: self.kept.supports_where.clone(),
            orientation: self.kept.orientation.clone(),
            custom: self.kept.custom.clone(),
        };
        s.validate()
            .map_err(|e| error_message(&e).unwrap_or((Field::Text, t("error-other"))))?;
        Ok(s)
    }

    fn duration_min(&self) -> Result<Option<u32>, (Field, String)> {
        if self.duration.trim().is_empty() {
            return Ok(None);
        }
        let r = model_data::DURATION_MIN;
        match parse_duration(&self.duration) {
            Some(m) if f64::from(m) <= r.max => Ok(Some(m)),
            Some(_) => Err(range_message(r.field, r.min, r.max)),
            None => Err((Field::Duration, t("form-error-duration"))),
        }
    }

    /// The settings fields fold away: a new log filled in from the
    /// recipe, not opened, with no setting in error.
    pub fn settings_folded(&self) -> bool {
        self.mode == FormMode::Log
            && self.prefilled
            && !self.settings_open
            && !matches!(
                self.error,
                Some((
                    Field::Layer
                        | Field::Nozzle
                        | Field::Bed
                        | Field::Hotend
                        | Field::Infill
                        | Field::Walls,
                    _
                ))
            )
    }

    fn outcome(&self) -> Result<AttemptResult, (Field, String)> {
        self.outcome
            .ok_or_else(|| (Field::Outcome, t("form-error-outcome")))
    }

    fn notes_text(&self) -> String {
        self.notes.text().trim_end().to_owned()
    }

    /// The new attempt (mode [`FormMode::Log`]).
    pub fn new_attempt(&self) -> Result<NewAttempt, (Field, String)> {
        let printed_at =
            parse_date(&self.date).ok_or_else(|| (Field::Date, t("form-error-date")))?;
        let result = self.outcome()?;
        Ok(NewAttempt {
            printed_at,
            result,
            settings: self.settings()?,
            duration_min: self.duration_min()?,
            notes: self.notes_text(),
            parts: Vec::new(),
        })
    }

    /// `a` with the form's changes (mode [`FormMode::Edit`]).
    pub fn updated(&self, a: &Attempt) -> Result<Attempt, (Field, String)> {
        let printed_at = if self.date.trim() == date_text(a.printed_at) {
            a.printed_at
        } else {
            parse_date(&self.date).ok_or_else(|| (Field::Date, t("form-error-date")))?
        };
        let result = self.outcome()?;
        Ok(Attempt {
            printed_at,
            result,
            settings: self.settings()?,
            duration_min: self.duration_min()?,
            notes: self.notes_text(),
            ..a.clone()
        })
    }

    /// The edited recipe (mode [`FormMode::Recipe`]); `None` when it is
    /// empty, which removes it.
    pub fn to_recipe(&self) -> Result<Option<Recipe>, (Field, String)> {
        let (settings, parts) = self.current()?;
        let mut notes = self.recipe_notes.clone();
        if let Some(text) = opt(&self.new_note) {
            notes.push(RecipeNote {
                material: None,
                text,
            });
        }
        let r = Recipe {
            settings,
            required: self.required.clone(),
            notes,
            parts,
            ..self.kept_recipe.clone()
        };
        if r.settings.is_empty() && r.notes.is_empty() && r.parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> Recipe {
        Recipe {
            settings: PrintSettings {
                material: Some(Material::Petg),
                layer_mm: Some(0.2),
                nozzle_mm: Some(0.4),
                bed_temp: Some(85),
                nozzle_temp: Some(240),
                printer: Some("MK4".into()),
                orientation: Some("flat".into()),
                ..PrintSettings::default()
            },
            ..Recipe::default()
        }
    }

    #[test]
    fn form_prefills_from_recipe() {
        let mut f = LogForm::log(Some(&recipe()), Some("Voron"));
        assert!(f.prefilled);
        // The recipe's settings fold into one line until opened.
        assert!(f.settings_folded());
        f.settings_open = true;
        assert!(!f.settings_folded());
        f.outcome = Some(AttemptResult::Success);
        assert_eq!(f.material, "PETG");
        assert_eq!(f.layer, "0.20");
        assert_eq!(f.nozzle, "0.4");
        assert_eq!(f.bed, "85");
        assert_eq!(f.printer, "MK4", "the recipe's printer wins");
        let a = f.new_attempt().unwrap();
        assert_eq!(a.settings.layer_mm, Some(0.2));
        assert_eq!(
            a.settings.orientation.as_deref(),
            Some("flat"),
            "hidden fields kept"
        );
        let blank = LogForm::log(None, Some("Voron"));
        assert!(!blank.prefilled);
        assert_eq!(blank.printer, "Voron");
    }

    #[test]
    fn form_rejects_out_of_range_with_message() {
        let mut f = LogForm::log(Some(&recipe()), None);
        // Nothing is picked until the user says how it went.
        assert_eq!(f.outcome, None);
        assert_eq!(f.new_attempt().unwrap_err().0, Field::Outcome);
        f.outcome = Some(AttemptResult::Failed);
        f.bed = "850".into();
        let (field, msg) = f.new_attempt().unwrap_err();
        assert_eq!(field, Field::Bed);
        assert!(msg.contains("150"), "{msg}");
        f.bed = "85".into();
        f.layer = "abc".into();
        assert_eq!(f.new_attempt().unwrap_err().0, Field::Layer);
        f.layer = "0,2".into();
        assert_eq!(f.new_attempt().unwrap().settings.layer_mm, Some(0.2));
        f.date = "2999-01-01".into();
        assert_eq!(f.new_attempt().unwrap_err().0, Field::Date);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("161"), Some(161));
        assert_eq!(parse_duration("2h 41m"), Some(161));
        assert_eq!(parse_duration("2:41"), Some(161));
        assert_eq!(parse_duration("2 h"), Some(120));
        assert_eq!(parse_duration("45m"), Some(45));
        assert_eq!(parse_duration("45 min"), Some(45));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(format_duration(161), "2 h 41 m");
        assert_eq!(format_duration(45), "45 m");
        assert_eq!(format_duration(120), "2 h");
    }

    #[test]
    fn empty_recipe_removes_it() {
        let f = LogForm::recipe(None, &[]);
        assert_eq!(f.to_recipe().unwrap(), None);
        let mut f = LogForm::recipe(Some(&recipe()), &[]);
        f.new_note = "Brim on the corners".into();
        let r = f.to_recipe().unwrap().unwrap();
        assert_eq!(r.notes.len(), 1);
        assert_eq!(r.settings.orientation.as_deref(), Some("flat"));
    }

    #[test]
    fn edit_keeps_date_unless_changed() {
        let a = Attempt {
            id: AttemptId::new(),
            printed_at: 1_700_000_000_123,
            result: AttemptResult::Failed,
            settings: PrintSettings::default(),
            duration_min: Some(90),
            notes: "Lifted".into(),
            parts: vec![],
            photos: vec![],
            number: 2,
            updated_at: 0,
        };
        let mut f = LogForm::edit(&a);
        assert_eq!(f.duration, "1 h 30 m");
        f.outcome = Some(AttemptResult::Partial);
        let b = f.updated(&a).unwrap();
        assert_eq!(b.printed_at, a.printed_at);
        assert_eq!(b.result, AttemptResult::Partial);
        assert_eq!(b.duration_min, Some(90));
        assert_eq!(b.notes, "Lifted");
    }

    fn parts() -> Vec<String> {
        vec!["body.stl".into(), "ring.stl".into()]
    }

    #[test]
    fn part_settings_are_edited_separately() {
        let mut f = LogForm::recipe(Some(&recipe()), &parts());
        assert!(f.has_part_choice());
        f.select_scope(Some("ring.stl".into())).unwrap();
        assert_eq!(f.layer, "", "a part starts with no settings of its own");
        assert_eq!(f.hint(crate::prints::Input::Layer), "0.20");
        f.layer = "0.12".into();
        f.walls = "4".into();
        assert!(f.part_has_settings("ring.stl"));
        assert!(!f.part_has_settings("body.stl"));
        // Back to the whole model: its fields come back untouched.
        f.select_scope(None).unwrap();
        assert_eq!(f.layer, "0.20");
        f.bed = "90".into();
        let r = f.to_recipe().unwrap().unwrap();
        assert_eq!(r.settings.bed_temp, Some(90));
        assert_eq!(r.settings.layer_mm, Some(0.2));
        assert_eq!(r.parts.len(), 1);
        assert_eq!(r.parts[0].part, "ring.stl");
        assert_eq!(r.parts[0].settings.layer_mm, Some(0.12));
        assert_eq!(r.parts[0].settings.walls, Some(4));
        assert_eq!(r.parts[0].settings.material, None, "only what differs");
    }

    #[test]
    fn saving_while_a_part_is_shown_keeps_both() {
        let mut f = LogForm::recipe(Some(&recipe()), &parts());
        f.select_scope(Some("body.stl".into())).unwrap();
        f.infill = "40".into();
        let r = f.to_recipe().unwrap().unwrap();
        assert_eq!(r.settings, recipe().settings);
        assert_eq!(r.parts[0].settings.infill_pct, Some(40));
    }

    #[test]
    fn cleared_part_loses_its_settings() {
        let mut saved = recipe();
        saved.parts = vec![PartOverride {
            part: "gone.stl".into(),
            settings: PrintSettings {
                layer_mm: Some(0.1),
                ..PrintSettings::default()
            },
        }];
        let mut f = LogForm::recipe(Some(&saved), &["body.stl".into()]);
        assert_eq!(
            f.parts,
            ["body.stl", "gone.stl"],
            "missing parts still listed"
        );
        f.select_scope(Some("gone.stl".into())).unwrap();
        assert_eq!(f.layer, "0.10");
        f.clear_part();
        assert!(!f.part_has_settings("gone.stl"));
        assert!(f.to_recipe().unwrap().unwrap().parts.is_empty());
    }

    #[test]
    fn invalid_part_value_blocks_switching() {
        let mut f = LogForm::recipe(Some(&recipe()), &parts());
        f.select_scope(Some("ring.stl".into())).unwrap();
        f.bed = "900".into();
        assert_eq!(f.select_scope(None).unwrap_err().0, Field::Bed);
        assert_eq!(f.scope.as_deref(), Some("ring.stl"));
    }

    #[test]
    fn single_part_models_have_no_part_choice() {
        let f = LogForm::recipe(Some(&recipe()), &["only.stl".into()]);
        assert!(!f.has_part_choice());
    }
}
