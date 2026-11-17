//! Catalogue numbers (№ 0412): per library, handed out in order, never
//! reused.

use crate::db::Db;
use crate::error::Result;
use crate::ids::{LibraryId, ModelId};

/// "№ 0412": at least four digits.
pub fn format_number(n: u32) -> String {
    format!("№ {n:04}")
}

/// A search for a catalogue number: `412`, `0412`, `№ 412`, `#412`,
/// `No. 412`. `exact` is true when the input was marked as a number (then
/// only the number matches); plain digits also match names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberQuery {
    pub number: u32,
    pub exact: bool,
}

pub fn parse_number_query(input: &str) -> Option<NumberQuery> {
    let s = input.trim();
    let lower = s.to_lowercase();
    let (rest, exact) = if let Some(r) = s.strip_prefix('№') {
        (r, true)
    } else if let Some(r) = s.strip_prefix('#') {
        (r, true)
    } else if lower.starts_with("no.") || lower.starts_with("no ") {
        (s.get(3..).unwrap_or_default(), true)
    } else {
        (s, false)
    };
    let digits = rest.trim();
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = digits.parse().ok().filter(|n| *n > 0)?;
    Some(NumberQuery { number, exact })
}

impl Db {
    /// Gives every present model of `lib` without a number the next ones,
    /// oldest first. Returns how many were numbered.
    pub async fn assign_numbers(&self, lib: LibraryId) -> Result<u64> {
        let mut tx = self.writer().begin().await?;
        let ids = sqlx::query_scalar!(
            r#"SELECT id as "id!: ModelId" FROM models
               WHERE library_id = ? AND number IS NULL
               ORDER BY created_at, rel_path"#,
            lib
        )
        .fetch_all(&mut *tx)
        .await?;
        if ids.is_empty() {
            return Ok(0);
        }
        let mut next = sqlx::query_scalar!(
            r#"SELECT next_number as "n!: i64" FROM libraries WHERE id = ?"#,
            lib
        )
        .fetch_one(&mut *tx)
        .await?;
        for id in &ids {
            // Skip numbers a sidecar claimed ahead of the counter.
            loop {
                let taken = sqlx::query_scalar!(
                    r#"SELECT 1 as "x!: i64" FROM models WHERE library_id = ? AND number = ?"#,
                    lib,
                    next
                )
                .fetch_optional(&mut *tx)
                .await?;
                if taken.is_none() {
                    break;
                }
                next += 1;
            }
            sqlx::query!("UPDATE models SET number = ? WHERE id = ?", next, id)
                .execute(&mut *tx)
                .await?;
            next += 1;
        }
        sqlx::query!(
            "UPDATE libraries SET next_number = ? WHERE id = ?",
            next,
            lib
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ids.len() as u64)
    }

    /// Gives model `id` number `n` from its sidecar, when the model has no
    /// number yet and no other model of its library has `n`. Returns
    /// whether it was taken.
    pub async fn claim_number(&self, id: ModelId, n: u32) -> Result<bool> {
        let n = i64::from(n);
        let mut tx = self.writer().begin().await?;
        let done = sqlx::query!(
            "UPDATE models SET number = ?1 WHERE id = ?2 AND number IS NULL
               AND NOT EXISTS (SELECT 1 FROM models o
                               WHERE o.library_id = models.library_id AND o.number = ?1)",
            n,
            id
        )
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query!(
            "UPDATE libraries SET next_number = max(next_number, ?1 + 1)
               WHERE id = (SELECT library_id FROM models WHERE id = ?2)",
            n,
            id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Model `id`'s catalogue number.
    pub async fn model_number(&self, id: ModelId) -> Result<Option<u32>> {
        let n = sqlx::query_scalar!(r#"SELECT number as "n?: i64" FROM models WHERE id = ?"#, id)
            .fetch_optional(self.reader())
            .await?
            .flatten();
        Ok(n.and_then(|n| u32::try_from(n).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_number_queries() {
        let q = |s| parse_number_query(s);
        assert_eq!(
            q("412"),
            Some(NumberQuery {
                number: 412,
                exact: false
            })
        );
        assert_eq!(
            q(" 0412 "),
            Some(NumberQuery {
                number: 412,
                exact: false
            })
        );
        assert_eq!(
            q("№ 412"),
            Some(NumberQuery {
                number: 412,
                exact: true
            })
        );
        assert_eq!(
            q("№0412"),
            Some(NumberQuery {
                number: 412,
                exact: true
            })
        );
        assert_eq!(
            q("#7"),
            Some(NumberQuery {
                number: 7,
                exact: true
            })
        );
        assert_eq!(
            q("No. 12"),
            Some(NumberQuery {
                number: 12,
                exact: true
            })
        );
        assert_eq!(
            q("no 12"),
            Some(NumberQuery {
                number: 12,
                exact: true
            })
        );
        assert_eq!(q("0"), None);
        assert_eq!(q("benchy"), None);
        assert_eq!(q("2x3"), None);
        assert_eq!(q("note"), None);
        assert_eq!(q("1234567890"), None);
    }

    #[test]
    fn formats_with_four_digits() {
        assert_eq!(format_number(7), "№ 0007");
        assert_eq!(format_number(412), "№ 0412");
        assert_eq!(format_number(12345), "№ 12345");
    }
}
