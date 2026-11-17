//! Full-text model search and library statistics.

use crate::db::Db;
use crate::error::Result;
use crate::grouping::ModelKind;
use crate::ids::{LibraryId, ModelId};

/// One search result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelHit {
    pub id: ModelId,
    pub library_id: LibraryId,
    pub name: String,
    pub rel_path: String,
    pub kind: ModelKind,
    /// Parts present on disk.
    pub part_count: u32,
    pub missing: bool,
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

/// Turns what the user typed into a safe FTS5 query: every whitespace-separated
/// token becomes a quoted prefix term, so FTS5 syntax is searched literally.
/// `None` when nothing searchable is left.
pub fn to_fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

struct HitRow {
    id: ModelId,
    library_id: LibraryId,
    name: String,
    rel_path: String,
    kind: String,
    part_count: i64,
    missing: bool,
}

impl From<HitRow> for ModelHit {
    fn from(r: HitRow) -> Self {
        ModelHit {
            id: r.id,
            library_id: r.library_id,
            name: r.name,
            rel_path: r.rel_path,
            kind: ModelKind::from_db(&r.kind).unwrap_or(ModelKind::Folder),
            part_count: u32::try_from(r.part_count).unwrap_or(u32::MAX),
            missing: r.missing,
        }
    }
}

impl Db {
    /// Models matching `input` (prefix match on name and path, name ranked
    /// higher), or all models by name when `input` is blank. Missing models
    /// are left out.
    pub async fn search_models(&self, input: &str, limit: u32) -> Result<Vec<ModelHit>> {
        let rows = match to_fts_query(input) {
            None => {
                sqlx::query_as!(
                    HitRow,
                    r#"SELECT m.id as "id!: ModelId", m.library_id as "library_id: LibraryId",
                              m.name, m.rel_path, m.kind,
                              (SELECT count(*) FROM parts p
                                WHERE p.model_id = m.id AND p.missing_since IS NULL) as "part_count!: i64",
                              m.missing_since IS NOT NULL as "missing!: bool"
                       FROM models m
                       WHERE m.missing_since IS NULL
                       ORDER BY m.name COLLATE NOCASE, m.rel_path
                       LIMIT ?"#,
                    limit
                )
                .fetch_all(self.reader())
                .await?
            }
            Some(q) => {
                sqlx::query_as!(
                    HitRow,
                    r#"SELECT m.id as "id!: ModelId", m.library_id as "library_id: LibraryId",
                              m.name, m.rel_path, m.kind,
                              (SELECT count(*) FROM parts p
                                WHERE p.model_id = m.id AND p.missing_since IS NULL) as "part_count!: i64",
                              m.missing_since IS NOT NULL as "missing!: bool"
                       FROM models_fts f JOIN models m ON m.rowid = f.rowid
                       WHERE models_fts MATCH ? AND m.missing_since IS NULL
                       ORDER BY bm25(models_fts, 10.0, 1.0), m.name COLLATE NOCASE
                       LIMIT ?"#,
                    q,
                    limit
                )
                .fetch_all(self.reader())
                .await?
            }
        };
        Ok(rows.into_iter().map(ModelHit::from).collect())
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
}
