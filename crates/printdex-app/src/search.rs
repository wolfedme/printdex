//! Search sequencing: every keystroke starts a query, and results of an
//! older query never replace those of a newer one.

use printdex_core::{LibraryId, ModelHit};

/// Maximum results per query; the status bar says when a result is capped.
pub const SEARCH_LIMIT: u32 = 5_000;

/// Identifies one query run. Carried by the async task and handed back with
/// its results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub generation: u64,
    pub text: String,
    pub scope: Option<LibraryId>,
}

/// Current query and its latest accepted results.
#[derive(Debug, Default)]
pub struct SearchState {
    pub text: String,
    pub scope: Option<LibraryId>,
    generation: u64,
    pub hits: Vec<ModelHit>,
    pub total: u64,
    pub loading: bool,
    /// Whether results have ever arrived (distinguishes "loading" from "empty").
    pub loaded: bool,
}

impl SearchState {
    /// Sets text and scope; returns a ticket when the query changed.
    pub fn begin(&mut self, text: &str, scope: Option<LibraryId>) -> Option<Ticket> {
        if self.loaded && text == self.text && scope == self.scope {
            return None;
        }
        self.text = text.to_owned();
        self.scope = scope;
        Some(self.refresh())
    }

    /// Re-runs the current query (after a scan, for example).
    pub fn refresh(&mut self) -> Ticket {
        self.generation += 1;
        self.loading = true;
        Ticket {
            generation: self.generation,
            text: self.text.clone(),
            scope: self.scope,
        }
    }

    /// Stores results for `ticket` unless a newer query has started.
    pub fn accept(&mut self, ticket: &Ticket, hits: Vec<ModelHit>, total: u64) -> bool {
        if ticket.generation != self.generation {
            return false;
        }
        self.hits = hits;
        self.total = total;
        self.loading = false;
        self.loaded = true;
        true
    }

    /// Whether more models match than were returned.
    pub fn capped(&self) -> bool {
        self.total > self.hits.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use printdex_core::{ModelId, ModelKind};

    fn hit(name: &str) -> ModelHit {
        ModelHit {
            id: ModelId::new(),
            library_id: LibraryId::new(),
            name: name.into(),
            rel_path: name.into(),
            kind: ModelKind::Folder,
            part_count: 1,
            missing: false,
        }
    }

    #[test]
    fn stale_results_are_dropped() {
        let mut s = SearchState::default();
        let t1 = s.begin("d", None).unwrap();
        let t2 = s.begin("dr", None).unwrap();
        assert!(!s.accept(&t1, vec![hit("Dog"), hit("Dragon")], 2));
        assert!(s.hits.is_empty());
        assert!(s.loading);
        assert!(s.accept(&t2, vec![hit("Dragon")], 1));
        assert_eq!(s.hits.len(), 1);
        assert!(!s.loading);
    }

    #[test]
    fn same_text_does_not_restart() {
        let mut s = SearchState::default();
        let t = s.begin("dragon", None).unwrap();
        s.accept(&t, vec![], 0);
        assert_eq!(s.begin("dragon", None), None);
        assert!(s.begin("dragon", Some(LibraryId::new())).is_some());
    }

    #[test]
    fn refresh_keeps_text_and_scope() {
        let mut s = SearchState::default();
        let lib = LibraryId::new();
        let t1 = s.begin("orc", Some(lib)).unwrap();
        let t2 = s.refresh();
        assert_eq!((t2.text.as_str(), t2.scope), ("orc", Some(lib)));
        assert!(t2.generation > t1.generation);
    }

    #[test]
    fn capped_when_total_exceeds_hits() {
        let mut s = SearchState::default();
        let t = s.begin("", None).unwrap();
        s.accept(&t, vec![hit("a")], 7);
        assert!(s.capped());
    }
}
