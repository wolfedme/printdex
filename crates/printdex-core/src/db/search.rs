//! Full-text model search and library statistics.

use std::collections::HashMap;

use sqlx::{QueryBuilder, Row, Sqlite};

use crate::db::Db;
use crate::db::numbers::{NumberQuery, parse_number_query};
use crate::error::Result;
use crate::grouping::ModelKind;
use crate::ids::{CollectionId, LibraryId, ModelId};
use crate::model_data::{AttemptResult, LastPrint, Material, PrintSettings, Supports, name_key};
use crate::time::now_ms;
use crate::walk::PrintableFormat;

/// What a smart filter keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Smart {
    /// No print attempt logged.
    NeverPrinted,
    /// The latest attempt failed.
    FailedLast,
    /// First seen in the last seven days.
    AddedThisWeek,
    /// No longer found in its library (kept while it has user data).
    Missing,
}

/// Length of "this week" for [`Smart::AddedThisWeek`].
pub const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Which models a search looks at (the sidebar selection).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scope {
    #[default]
    All,
    Library(LibraryId),
    Collection(CollectionId),
    /// Models with this tag (matched ignoring case).
    Tag(String),
    Smart(Smart),
}

/// The recipe facts cards show.
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeFacts {
    pub material: Option<Material>,
    pub supports: Option<Supports>,
    pub layer_mm: Option<f32>,
}

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelHit {
    pub id: ModelId,
    pub library_id: LibraryId,
    pub name: String,
    pub rel_path: String,
    pub kind: ModelKind,
    /// Catalogue number in its library (№ 0412).
    pub number: Option<u32>,
    /// Parts present on disk.
    pub part_count: u32,
    pub missing: bool,
    /// When Printdex first saw the model, unix milliseconds.
    pub created_at: i64,
    /// The material previews are drawn in: the recipe's, else the latest
    /// attempt's.
    pub material: Option<Material>,
    pub recipe: Option<RecipeFacts>,
    pub last_print: Option<LastPrint>,
}

/// Row counts across all libraries. `models`, `parts` and `assets` count
/// rows present on disk; missing models are counted separately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub libraries: u64,
    pub models: u64,
    pub parts: u64,
    pub assets: u64,
    pub missing_models: u64,
}

/// Model counts for the sidebar's smart filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SmartCounts {
    pub never_printed: u64,
    pub failed_last: u64,
    pub added_this_week: u64,
    pub missing: u64,
}

/// Structured narrowing of a search. The default narrows nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchFilter {
    /// Keep models with at least one present part in one of these formats.
    /// Empty keeps every model.
    pub formats: Vec<PrintableFormat>,
    /// `Some(true)`: two or more present parts; `Some(false)`: at most one.
    pub multi_part: Option<bool>,
    /// Keep models whose material (recipe, else latest attempt) is one of
    /// these. Empty keeps every model.
    pub materials: Vec<Material>,
}

impl SearchFilter {
    pub fn is_empty(&self) -> bool {
        self.formats.is_empty() && self.multi_part.is_none() && self.materials.is_empty()
    }
}

/// Result order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortOrder {
    /// Relevance for text searches, name for blank searches.
    #[default]
    Name,
    RecentlyAdded,
    /// Order of adding, inside a collection; name elsewhere.
    Collection,
}

/// Turns what the user typed into a safe FTS5 query: every whitespace-separated
/// token becomes a quoted prefix term, so FTS5 syntax is searched literally.
/// `None` when nothing searchable is left.
pub fn to_fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        // Tokens without a letter or digit ("*", "-", ".") match nothing
        // useful and confuse FTS5; drop them.
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// What a search matches on.
enum Text {
    /// Blank: everything.
    All,
    Fts(String),
    /// A catalogue number; plain digits also match text.
    Number(NumberQuery, Option<String>),
}

fn text_query(input: &str) -> Text {
    match (parse_number_query(input), to_fts_query(input)) {
        (Some(n), _) if n.exact => Text::Number(n, None),
        (Some(n), fts) => Text::Number(n, fts),
        (None, Some(fts)) => Text::Fts(fts),
        (None, None) => Text::All,
    }
}

/// SQL for a model's latest attempt column `col` (correlated on `m`).
fn latest(col: &str) -> String {
    format!(
        "(SELECT a.{col} FROM attempts a WHERE a.model_id = m.id ORDER BY a.printed_at DESC, a.id DESC LIMIT 1)"
    )
}

/// SQL for a model's preview material key.
fn material_sql() -> String {
    "coalesce((SELECT r.material FROM recipes r WHERE r.model_id = m.id), \
     (SELECT a.material FROM attempts a WHERE a.model_id = m.id AND a.material IS NOT NULL \
      ORDER BY a.printed_at DESC, a.id DESC LIMIT 1))"
        .to_owned()
}

/// Pushes `FROM … WHERE …` for a search: present models in `scope`,
/// matching `fts` and `filter`.
fn push_from_where(
    q: &mut QueryBuilder<Sqlite>,
    scope: &Scope,
    text: &Text,
    filter: &SearchFilter,
) {
    match text {
        Text::Fts(_) => q.push(" FROM models_fts f JOIN models m ON m.rowid = f.rowid"),
        _ => q.push(" FROM models m"),
    };
    if let Scope::Collection(c) = scope {
        q.push(" JOIN collection_models cm ON cm.model_id = m.id AND cm.collection_id = ")
            .push_bind(*c);
    }
    if *scope == Scope::Smart(Smart::Missing) {
        q.push(" WHERE m.missing_since IS NOT NULL");
    } else {
        q.push(" WHERE m.missing_since IS NULL");
    }
    match text {
        Text::All => {}
        Text::Fts(fts) => {
            q.push(" AND models_fts MATCH ").push_bind(fts.clone());
        }
        Text::Number(n, None) => {
            q.push(" AND m.number = ").push_bind(i64::from(n.number));
        }
        Text::Number(n, Some(fts)) => {
            q.push(" AND (m.number = ")
                .push_bind(i64::from(n.number))
                .push(" OR m.rowid IN (SELECT rowid FROM models_fts WHERE models_fts MATCH ")
                .push_bind(fts.clone())
                .push("))");
        }
    }
    match scope {
        Scope::All | Scope::Collection(_) => {}
        Scope::Library(id) => {
            q.push(" AND m.library_id = ").push_bind(*id);
        }
        Scope::Tag(name) => {
            q.push(
                " AND m.id IN (SELECT mt.model_id FROM model_tags mt JOIN tags t ON t.id = mt.tag_id WHERE t.name_key = ",
            )
            .push_bind(name_key(name))
            .push(")");
        }
        Scope::Smart(Smart::NeverPrinted) => {
            q.push(" AND NOT EXISTS (SELECT 1 FROM attempts a WHERE a.model_id = m.id)");
        }
        Scope::Smart(Smart::FailedLast) => {
            q.push(format!(" AND {} = 'failed'", latest("result")));
        }
        Scope::Smart(Smart::AddedThisWeek) => {
            q.push(" AND m.created_at >= ")
                .push_bind(now_ms() - WEEK_MS);
        }
        Scope::Smart(Smart::Missing) => {}
    }
    if !filter.formats.is_empty() {
        q.push(" AND EXISTS (SELECT 1 FROM parts p WHERE p.model_id = m.id AND p.missing_since IS NULL AND p.format IN (");
        let mut sep = q.separated(", ");
        for f in &filter.formats {
            sep.push_bind(f.as_str());
        }
        q.push("))");
    }
    if let Some(multi) = filter.multi_part {
        q.push(" AND ((SELECT count(*) FROM parts p WHERE p.model_id = m.id AND p.missing_since IS NULL) >= 2) = ")
            .push_bind(multi);
    }
    if !filter.materials.is_empty() {
        q.push(format!(" AND {} IN (", material_sql()));
        let mut sep = q.separated(", ");
        for mat in &filter.materials {
            sep.push_bind(mat.key());
        }
        q.push(")");
    }
}

fn hit_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ModelHit> {
    let kind: String = row.try_get("kind")?;
    let recipe: Option<String> = row.try_get("recipe")?;
    let last_result: Option<String> = row.try_get("last_result")?;
    let last_at: Option<i64> = row.try_get("last_at")?;
    let successes: i64 = row.try_get("successes")?;
    let material: Option<String> = row.try_get("material")?;
    let part_count: i64 = row.try_get("part_count")?;
    let number: Option<i64> = row.try_get("number")?;
    let recipe = recipe.map(|s| {
        let settings: PrintSettings = serde_json::from_str(&s).unwrap_or_default();
        RecipeFacts {
            material: settings.material,
            supports: settings.supports,
            layer_mm: settings.layer_mm,
        }
    });
    let last_print = match (
        last_result.as_deref().and_then(AttemptResult::from_db),
        last_at,
    ) {
        (Some(result), Some(at)) => Some(LastPrint {
            result,
            at,
            successes: u32::try_from(successes).unwrap_or(0),
        }),
        _ => None,
    };
    // The recipe's own spelling wins over the stored lower-case key.
    let material = recipe
        .as_ref()
        .and_then(|r| r.material.clone())
        .or_else(|| material.as_deref().and_then(Material::parse));
    Ok(ModelHit {
        id: row.try_get("id")?,
        library_id: row.try_get("library_id")?,
        name: row.try_get("name")?,
        rel_path: row.try_get("rel_path")?,
        kind: ModelKind::from_db(&kind).unwrap_or(ModelKind::Folder),
        number: number.and_then(|n| u32::try_from(n).ok()),
        part_count: u32::try_from(part_count).unwrap_or(u32::MAX),
        missing: row.try_get::<Option<i64>, _>("missing_since")?.is_some(),
        created_at: row.try_get("created_at")?,
        material,
        recipe,
        last_print,
    })
}

impl Db {
    /// Models matching `input` across all libraries, by name; see
    /// [`Db::search_models_in`].
    pub async fn search_models(&self, input: &str, limit: u32) -> Result<Vec<ModelHit>> {
        self.search_models_in(
            &Scope::All,
            input,
            &SearchFilter::default(),
            SortOrder::Name,
            limit,
        )
        .await
    }

    /// Models matching `input` (prefix match on name, path, tags, notes and
    /// recipe text, name ranked highest), or all models when `input` is
    /// blank, in `scope` and narrowed by `filter`. With [`SortOrder::Name`],
    /// text searches are ranked by relevance and blank searches sorted by
    /// name; [`SortOrder::RecentlyAdded`] puts the newest first;
    /// [`SortOrder::Collection`] keeps a collection's order. Missing models
    /// are left out.
    pub async fn search_models_in(
        &self,
        scope: &Scope,
        input: &str,
        filter: &SearchFilter,
        sort: SortOrder,
        limit: u32,
    ) -> Result<Vec<ModelHit>> {
        let text = text_query(input);
        let mut q = QueryBuilder::<Sqlite>::new(format!(
            "SELECT m.id, m.library_id, m.name, m.rel_path, m.kind, m.created_at, m.number, m.missing_since,
                    (SELECT count(*) FROM parts p WHERE p.model_id = m.id AND p.missing_since IS NULL) as part_count,
                    (SELECT r.settings FROM recipes r WHERE r.model_id = m.id) as recipe,
                    {} as last_result, {} as last_at,
                    (SELECT count(*) FROM attempts a WHERE a.model_id = m.id AND a.result = 'success') as successes,
                    {} as material",
            latest("result"),
            latest("printed_at"),
            material_sql()
        ));
        push_from_where(&mut q, scope, &text, filter);
        q.push(" ORDER BY ");
        if let Text::Number(n, _) = &text {
            // The model with that number first.
            q.push("(m.number = ")
                .push_bind(i64::from(n.number))
                .push(") DESC, ");
        }
        match (sort, scope, &text) {
            (SortOrder::Collection, Scope::Collection(_), _) => q.push("cm.position, "),
            (SortOrder::RecentlyAdded, _, _) => q.push("m.created_at DESC, "),
            (_, _, Text::Fts(_)) => q.push("bm25(models_fts, 10.0, 1.0, 4.0, 1.0, 1.0), "),
            _ => q.push(""),
        };
        q.push("m.name COLLATE NOCASE, m.rel_path LIMIT ")
            .push_bind(i64::from(limit));
        let rows = q.build().fetch_all(self.reader()).await?;
        rows.iter().map(hit_from_row).collect()
    }

    /// How many present models match (all of them when `input` is blank),
    /// with the same narrowing as search and no limit.
    pub async fn count_models(
        &self,
        scope: &Scope,
        input: &str,
        filter: &SearchFilter,
    ) -> Result<u64> {
        let text = text_query(input);
        let mut q = QueryBuilder::<Sqlite>::new("SELECT count(*)");
        push_from_where(&mut q, scope, &text, filter);
        let n: i64 = q.build_query_scalar().fetch_one(self.reader()).await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Present-model counts for the smart filters.
    pub async fn smart_counts(&self) -> Result<SmartCounts> {
        let mut out = SmartCounts::default();
        for (smart, slot) in [
            (Smart::NeverPrinted, &mut out.never_printed),
            (Smart::FailedLast, &mut out.failed_last),
            (Smart::AddedThisWeek, &mut out.added_this_week),
            (Smart::Missing, &mut out.missing),
        ] {
            *slot = self
                .count_models(&Scope::Smart(smart), "", &SearchFilter::default())
                .await?;
        }
        Ok(out)
    }

    /// Present-model counts per library. Libraries without models are absent.
    pub async fn library_model_counts(&self) -> Result<HashMap<LibraryId, u64>> {
        let rows = sqlx::query!(
            r#"SELECT library_id as "library_id!: LibraryId", count(*) as "n!: i64"
               FROM models WHERE missing_since IS NULL GROUP BY library_id"#
        )
        .fetch_all(self.reader())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.library_id, u64::try_from(r.n).unwrap_or(0)))
            .collect())
    }

    /// Row counts.
    pub async fn stats(&self) -> Result<Stats> {
        let r = sqlx::query!(
            r#"SELECT
                 (SELECT count(*) FROM libraries) as "libraries!: i64",
                 (SELECT count(*) FROM models WHERE missing_since IS NULL) as "models!: i64",
                 (SELECT count(*) FROM parts WHERE missing_since IS NULL) as "parts!: i64",
                 (SELECT count(*) FROM assets WHERE missing_since IS NULL) as "assets!: i64",
                 (SELECT count(*) FROM models WHERE missing_since IS NOT NULL) as "missing_models!: i64""#
        )
        .fetch_one(self.reader())
        .await?;
        let n = |v: i64| u64::try_from(v).unwrap_or(0);
        Ok(Stats {
            libraries: n(r.libraries),
            models: n(r.models),
            parts: n(r.parts),
            assets: n(r.assets),
            missing_models: n(r.missing_models),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Library, LibraryKind};
    use crate::ids::{AssetId, PartId};
    use std::path::PathBuf;

    struct Fx {
        _d: tempfile::TempDir,
        db: Db,
        lib: LibraryId,
    }

    async fn fx() -> Fx {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open(&d.path().join("t.db")).await.unwrap();
        let lib = Library {
            id: LibraryId::new(),
            path: PathBuf::from("/lib"),
            display_name: "lib".into(),
            kind: LibraryKind::Local,
            change_mode: crate::db::ChangeMode::Auto,
            sidecar_enabled: true,
            last_full_scan_at: None,
        };
        db.insert_library(&lib).await.unwrap();
        Fx {
            _d: d,
            db,
            lib: lib.id,
        }
    }

    impl Fx {
        async fn model(&self, rel_path: &str, name: &str, missing: bool) -> ModelId {
            let id = ModelId::new();
            sqlx::query(
                "INSERT INTO models (id, library_id, rel_path, name, kind, missing_since, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'folder', ?, 1, 1)",
            )
            .bind(id)
            .bind(self.lib)
            .bind(rel_path)
            .bind(name)
            .bind(missing.then_some(5_i64))
            .execute(self.db.writer())
            .await
            .unwrap();
            id
        }

        async fn part(&self, model: ModelId, rel_path: &str) {
            sqlx::query(
                "INSERT INTO parts (id, model_id, library_id, rel_path, format, size, mtime_ns, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'stl', 1, 1, 1, 1)",
            )
            .bind(PartId::new())
            .bind(model)
            .bind(self.lib)
            .bind(rel_path)
            .execute(self.db.writer())
            .await
            .unwrap();
        }

        async fn asset(&self, model: ModelId, rel_path: &str) {
            sqlx::query(
                "INSERT INTO assets (id, model_id, library_id, rel_path, size, mtime_ns, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 1, 1, 1, 1)",
            )
            .bind(AssetId::new())
            .bind(model)
            .bind(self.lib)
            .bind(rel_path)
            .execute(self.db.writer())
            .await
            .unwrap();
        }

        async fn names(&self, q: &str, limit: u32) -> Vec<String> {
            self.db
                .search_models(q, limit)
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.name)
                .collect()
        }
    }

    #[test]
    fn fts_query_escapes_syntax() {
        assert_eq!(
            to_fts_query("dragon (v2)").as_deref(),
            Some("\"dragon\"* \"(v2)\"*")
        );
        assert_eq!(
            to_fts_query("say \"hi\" -x AND").as_deref(),
            Some("\"say\"* \"hi\"* \"-x\"* \"AND\"*")
        );
        assert_eq!(to_fts_query("   "), None);
        assert_eq!(to_fts_query("\" \"\""), None);
    }

    #[test]
    fn symbol_only_search_tokens_are_dropped() {
        let plain = to_fts_query("dragon");
        assert_eq!(to_fts_query("dragon *"), plain);
        assert_eq!(to_fts_query("dragon -"), plain);
        assert_eq!(to_fts_query("dragon . -- **"), plain);
        assert_eq!(to_fts_query("* -"), None);
    }

    #[tokio::test]
    async fn search_prefix_matches_name() {
        let f = fx().await;
        f.model("Red Dragon", "Red Dragon", false).await;
        f.model("Orc Warrior", "Orc Warrior", false).await;
        assert_eq!(f.names("drag", 50).await, ["Red Dragon"]);
    }

    #[tokio::test]
    async fn search_matches_path_segments() {
        let f = fx().await;
        f.model("Minis/Dragons/Red", "Red", false).await;
        assert_eq!(f.names("minis", 50).await, ["Red"]);
    }

    #[tokio::test]
    async fn search_is_diacritic_insensitive() {
        let f = fx().await;
        f.model("Drachen_Ü", "Drachen_Ü", false).await;
        assert_eq!(f.names("drachen u", 50).await, ["Drachen_Ü"]);
    }

    #[tokio::test]
    async fn search_ranks_name_over_path() {
        let f = fx().await;
        f.model("Dragon Pack/Knight", "Knight", false).await;
        f.model("Misc/Dragon", "Dragon", false).await;
        assert_eq!(f.names("dragon", 50).await, ["Dragon", "Knight"]);
    }

    #[tokio::test]
    async fn blank_search_lists_all_sorted() {
        let f = fx().await;
        for n in ["beta", "Alpha", "gamma"] {
            f.model(n, n, false).await;
        }
        assert_eq!(f.names("", 50).await, ["Alpha", "beta", "gamma"]);
        assert_eq!(f.names("  ", 2).await, ["Alpha", "beta"]);
    }

    #[tokio::test]
    async fn missing_models_excluded() {
        let f = fx().await;
        f.model("Gone Dragon", "Gone Dragon", true).await;
        f.model("Red Dragon", "Red Dragon", false).await;
        assert_eq!(f.names("dragon", 50).await, ["Red Dragon"]);
        assert_eq!(f.names("", 50).await, ["Red Dragon"]);
        assert_eq!(f.db.stats().await.unwrap().missing_models, 1);
    }

    #[tokio::test]
    async fn hits_carry_part_count() {
        let f = fx().await;
        let m = f.model("Dragon", "Dragon", false).await;
        f.part(m, "Dragon/a.stl").await;
        f.part(m, "Dragon/b.stl").await;
        let hits = f.db.search_models("dragon", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, m);
        assert_eq!(hits[0].library_id, f.lib);
        assert_eq!(hits[0].rel_path, "Dragon");
        assert_eq!(hits[0].kind, ModelKind::Folder);
        assert_eq!(hits[0].part_count, 2);
        assert!(!hits[0].missing);
    }

    #[tokio::test]
    async fn stats_counts_rows() {
        let f = fx().await;
        let a = f.model("A", "A", false).await;
        let b = f.model("B", "B", false).await;
        f.part(a, "A/1.stl").await;
        f.part(a, "A/2.stl").await;
        f.part(b, "B/1.stl").await;
        f.asset(a, "A/x.jpg").await;
        assert_eq!(
            f.db.stats().await.unwrap(),
            Stats {
                libraries: 1,
                models: 2,
                parts: 3,
                assets: 1,
                missing_models: 0
            }
        );
    }

    impl Fx {
        async fn second_library(&self) -> LibraryId {
            self.second_library_named("/lib2").await
        }

        async fn model_in(&self, lib: LibraryId, rel_path: &str) -> ModelId {
            let id = ModelId::new();
            sqlx::query(
                "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'folder', 1, 1)",
            )
            .bind(id)
            .bind(lib)
            .bind(rel_path)
            .bind(rel_path)
            .execute(self.db.writer())
            .await
            .unwrap();
            id
        }
    }

    #[tokio::test]
    async fn search_filters_by_library() {
        let f = fx().await;
        let other = f.second_library().await;
        let a = f.model_in(f.lib, "Dragon").await;
        f.model_in(other, "Dragon").await;
        let hits =
            f.db.search_models_in(
                &Scope::Library(f.lib),
                "dragon",
                &SearchFilter::default(),
                SortOrder::Name,
                50,
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, a);
        let all =
            f.db.search_models_in(
                &Scope::Library(other),
                "",
                &SearchFilter::default(),
                SortOrder::Name,
                50,
            )
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            f.db.search_models_in(
                &Scope::All,
                "dragon",
                &SearchFilter::default(),
                SortOrder::Name,
                50
            )
            .await
            .unwrap()
            .len(),
            2
        );
    }

    #[tokio::test]
    async fn count_matches_search() {
        let f = fx().await;
        let other = f.second_library().await;
        for n in ["Red Dragon", "Blue Dragon", "Orc"] {
            f.model(n, n, false).await;
        }
        f.model("Gone Dragon", "Gone Dragon", true).await;
        f.model_in(other, "Green Dragon").await;
        assert_eq!(
            f.db.count_models(&Scope::All, "drag", &SearchFilter::default())
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            f.db.count_models(&Scope::Library(f.lib), "drag", &SearchFilter::default())
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            f.db.count_models(&Scope::All, "  ", &SearchFilter::default())
                .await
                .unwrap(),
            4
        );
        assert_eq!(
            f.db.count_models(&Scope::Library(other), "", &SearchFilter::default())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn library_counts_per_library() {
        let f = fx().await;
        let other = f.second_library().await;
        let _empty = f.second_library_named("/lib3").await;
        f.model("A", "A", false).await;
        f.model("B", "B", false).await;
        f.model("C", "C", true).await;
        f.model_in(other, "X").await;
        let counts = f.db.library_model_counts().await.unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[&f.lib], 2);
        assert_eq!(counts[&other], 1);
    }

    impl Fx {
        async fn second_library_named(&self, path: &str) -> LibraryId {
            let lib = Library {
                id: LibraryId::new(),
                path: PathBuf::from(path),
                display_name: path.into(),
                kind: LibraryKind::Local,
                change_mode: crate::db::ChangeMode::Auto,
                sidecar_enabled: true,
                last_full_scan_at: None,
            };
            self.db.insert_library(&lib).await.unwrap();
            lib.id
        }
    }

    impl Fx {
        async fn part_fmt(&self, model: ModelId, rel_path: &str, format: &str) {
            sqlx::query(
                "INSERT INTO parts (id, model_id, library_id, rel_path, format, size, mtime_ns, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, 1, 1, 1, 1)",
            )
            .bind(PartId::new())
            .bind(model)
            .bind(self.lib)
            .bind(rel_path)
            .bind(format)
            .execute(self.db.writer())
            .await
            .unwrap();
        }

        async fn hits(&self, q: &str, filter: &SearchFilter, sort: SortOrder) -> Vec<String> {
            self.db
                .search_models_in(&Scope::All, q, filter, sort, 50)
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.name)
                .collect()
        }
    }

    #[tokio::test]
    async fn search_filters_by_format_and_parts() {
        let f = fx().await;
        let bin = f.model("Bin", "Bin", false).await;
        f.part_fmt(bin, "Bin/a.stl", "stl").await;
        f.part_fmt(bin, "Bin/b.3mf", "3mf").await;
        let hook = f.model("Hook", "Hook", false).await;
        f.part_fmt(hook, "Hook/h.stl", "stl").await;
        let vase = f.model("Vase", "Vase", false).await;
        f.part_fmt(vase, "Vase/v.obj", "obj").await;

        let three_mf = SearchFilter {
            formats: vec![PrintableFormat::ThreeMf],
            multi_part: None,
            ..SearchFilter::default()
        };
        assert_eq!(f.hits("", &three_mf, SortOrder::Name).await, ["Bin"]);
        let stl_or_obj = SearchFilter {
            formats: vec![PrintableFormat::Stl, PrintableFormat::Obj],
            multi_part: None,
            ..SearchFilter::default()
        };
        assert_eq!(
            f.hits("", &stl_or_obj, SortOrder::Name).await,
            ["Bin", "Hook", "Vase"]
        );
        let multi = SearchFilter {
            formats: vec![],
            multi_part: Some(true),
            ..SearchFilter::default()
        };
        assert_eq!(f.hits("", &multi, SortOrder::Name).await, ["Bin"]);
        let single = SearchFilter {
            formats: vec![PrintableFormat::Stl],
            multi_part: Some(false),
            ..SearchFilter::default()
        };
        assert_eq!(f.hits("", &single, SortOrder::Name).await, ["Hook"]);
        assert_eq!(f.hits("ho", &single, SortOrder::Name).await, ["Hook"]);
        assert_eq!(
            f.db.count_models(&Scope::All, "", &single).await.unwrap(),
            1
        );
        assert_eq!(f.db.count_models(&Scope::All, "", &multi).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn sort_by_recently_added() {
        let f = fx().await;
        for (name, at) in [("Old", 1_i64), ("New", 3), ("Mid", 2)] {
            sqlx::query(
                "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'folder', ?, ?)",
            )
            .bind(ModelId::new())
            .bind(f.lib)
            .bind(name)
            .bind(name)
            .bind(at)
            .bind(at)
            .execute(f.db.writer())
            .await
            .unwrap();
        }
        let none = SearchFilter::default();
        assert_eq!(
            f.hits("", &none, SortOrder::RecentlyAdded).await,
            ["New", "Mid", "Old"]
        );
        assert_eq!(
            f.hits("", &none, SortOrder::Name).await,
            ["Mid", "New", "Old"]
        );
    }

    fn log(
        at: i64,
        result: crate::model_data::AttemptResult,
        mat: Material,
    ) -> crate::model_data::NewAttempt {
        crate::model_data::NewAttempt {
            printed_at: at,
            result,
            settings: PrintSettings {
                material: Some(mat),
                ..PrintSettings::default()
            },
            duration_min: None,
            notes: String::new(),
            parts: vec![],
        }
    }

    async fn scoped(f: &Fx, scope: Scope, sort: SortOrder) -> Vec<String> {
        let names: Vec<String> =
            f.db.search_models_in(&scope, "", &SearchFilter::default(), sort, 50)
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.name)
                .collect();
        let n =
            f.db.count_models(&scope, "", &SearchFilter::default())
                .await
                .unwrap();
        assert_eq!(
            n as usize,
            names.len(),
            "count matches search for {scope:?}"
        );
        names
    }

    #[tokio::test]
    async fn scope_collection_in_collection_order() {
        let f = fx().await;
        let c = f.db.create_collection("Gifts").await.unwrap();
        let z = f.model("Zebra", "Zebra", false).await;
        let a = f.model("Ant", "Ant", false).await;
        f.model("Other", "Other", false).await;
        f.db.add_to_collection(c.id, z).await.unwrap();
        f.db.add_to_collection(c.id, a).await.unwrap();
        assert_eq!(
            scoped(&f, Scope::Collection(c.id), SortOrder::Collection).await,
            ["Zebra", "Ant"]
        );
        assert_eq!(
            scoped(&f, Scope::Collection(c.id), SortOrder::Name).await,
            ["Ant", "Zebra"]
        );
    }

    #[tokio::test]
    async fn scope_tag() {
        let f = fx().await;
        let a = f.model("A", "A", false).await;
        f.model("B", "B", false).await;
        f.db.add_tag(a, "Gridfinity").await.unwrap();
        assert_eq!(
            scoped(&f, Scope::Tag("gridfinity".into()), SortOrder::Name).await,
            ["A"]
        );
        assert!(
            scoped(&f, Scope::Tag("nope".into()), SortOrder::Name)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn smart_filters() {
        use crate::model_data::AttemptResult::*;
        let f = fx().await;
        let printed = f.model("Printed", "Printed", false).await;
        let failed = f.model("Failed", "Failed", false).await;
        f.model("Never", "Never", false).await;
        f.db.log_attempt(printed, &log(1, Failed, Material::Pla))
            .await
            .unwrap();
        f.db.log_attempt(printed, &log(2, Success, Material::Pla))
            .await
            .unwrap();
        f.db.log_attempt(failed, &log(5, Failed, Material::Petg))
            .await
            .unwrap();
        assert_eq!(
            scoped(&f, Scope::Smart(Smart::NeverPrinted), SortOrder::Name).await,
            ["Never"]
        );
        assert_eq!(
            scoped(&f, Scope::Smart(Smart::FailedLast), SortOrder::Name).await,
            ["Failed"]
        );
        // Fixture models were created at t=1: long ago.
        assert!(
            scoped(&f, Scope::Smart(Smart::AddedThisWeek), SortOrder::Name)
                .await
                .is_empty()
        );
        sqlx::query("UPDATE models SET created_at = ? WHERE id = ?")
            .bind(now_ms() - 1000)
            .bind(failed)
            .execute(f.db.writer())
            .await
            .unwrap();
        assert_eq!(
            scoped(&f, Scope::Smart(Smart::AddedThisWeek), SortOrder::Name).await,
            ["Failed"]
        );
        assert_eq!(
            f.db.smart_counts().await.unwrap(),
            SmartCounts {
                never_printed: 1,
                failed_last: 1,
                added_this_week: 1,
                missing: 0
            }
        );
    }

    #[tokio::test]
    async fn hits_carry_recipe_and_last_print() {
        use crate::model_data::AttemptResult::*;
        let f = fx().await;
        let a = f.model("A", "A", false).await;
        let b = f.model("B", "B", false).await;
        f.db.log_attempt(a, &log(10, Success, Material::Tpu))
            .await
            .unwrap();
        f.db.log_attempt(a, &log(20, Success, Material::Tpu))
            .await
            .unwrap();
        f.db.log_attempt(a, &log(30, Partial, Material::Tpu))
            .await
            .unwrap();
        let recipe = crate::model_data::Recipe {
            settings: PrintSettings {
                material: Some(Material::Petg),
                supports: Some(Supports::Tree),
                layer_mm: Some(0.2),
                ..PrintSettings::default()
            },
            ..crate::model_data::Recipe::default()
        };
        f.db.set_recipe(b, Some(&recipe)).await.unwrap();
        let hits = f.db.search_models("", 10).await.unwrap();
        assert_eq!(hits[0].material, Some(Material::Tpu));
        assert_eq!(
            hits[0].last_print,
            Some(LastPrint {
                result: Partial,
                at: 30,
                successes: 2
            })
        );
        assert_eq!(hits[0].recipe, None);
        assert_eq!(hits[1].material, Some(Material::Petg));
        assert_eq!(
            hits[1].recipe,
            Some(RecipeFacts {
                material: Some(Material::Petg),
                supports: Some(Supports::Tree),
                layer_mm: Some(0.2)
            })
        );
        assert_eq!(hits[1].last_print, None);
    }

    #[tokio::test]
    async fn material_filter() {
        use crate::model_data::AttemptResult::*;
        let f = fx().await;
        let a = f.model("A", "A", false).await;
        let b = f.model("B", "B", false).await;
        f.model("C", "C", false).await;
        f.db.log_attempt(a, &log(1, Success, Material::Petg))
            .await
            .unwrap();
        f.db.log_attempt(b, &log(1, Success, Material::Other("Wood fill".into())))
            .await
            .unwrap();
        let petg = SearchFilter {
            materials: vec![Material::Petg],
            ..SearchFilter::default()
        };
        assert_eq!(f.hits("", &petg, SortOrder::Name).await, ["A"]);
        let wood = SearchFilter {
            materials: vec![Material::Other("wood FILL".into())],
            ..SearchFilter::default()
        };
        assert_eq!(f.hits("", &wood, SortOrder::Name).await, ["B"]);
        assert_eq!(f.db.count_models(&Scope::All, "", &petg).await.unwrap(), 1);
    }
}
