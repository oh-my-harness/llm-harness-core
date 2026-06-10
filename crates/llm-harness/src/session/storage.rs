use std::collections::HashMap;
use std::sync::Mutex;

use futures::future::BoxFuture;
use llm_harness_types::{EntryId, SessionError};

use super::types::{SessionEntry, SessionEntryKind, SessionMetadata};

// ── SessionStorage trait ───────────────────────────────────────────────────────

/// Low-level storage backend for a single session.
///
/// Implementations must serialize all writes internally so callers can share
/// `Arc<dyn SessionStorage>` freely.
pub trait SessionStorage: Send + Sync {
    fn metadata(&self) -> BoxFuture<'_, Result<SessionMetadata, SessionError>>;

    /// Generate a new time-ordered entry ID (sync — no I/O needed).
    fn create_entry_id(&self) -> EntryId;

    fn append_entry(&self, entry: SessionEntry) -> BoxFuture<'_, Result<(), SessionError>>;

    fn get_entry(&self, id: EntryId) -> BoxFuture<'_, Result<Option<SessionEntry>, SessionError>>;

    fn children(&self, parent: EntryId) -> BoxFuture<'_, Result<Vec<SessionEntry>, SessionError>>;

    fn all_leaves(&self) -> BoxFuture<'_, Result<Vec<EntryId>, SessionError>>;

    fn active_cursor(&self) -> BoxFuture<'_, Result<Option<EntryId>, SessionError>>;

    fn set_active_cursor(&self, id: EntryId) -> BoxFuture<'_, Result<(), SessionError>>;

    /// Return all entries from `target` to the root, ordered root-first.
    fn path_to_root(
        &self,
        target: EntryId,
    ) -> BoxFuture<'_, Result<Vec<SessionEntry>, SessionError>>;

    fn common_ancestor(
        &self,
        a: EntryId,
        b: EntryId,
    ) -> BoxFuture<'_, Result<Option<EntryId>, SessionError>>;

    /// Return the label name for the entry at `id`, if any.
    fn label_at(&self, id: EntryId) -> BoxFuture<'_, Result<Option<String>, SessionError>>;

    fn find_entries_by_type(
        &self,
        kind: SessionEntryKind,
    ) -> BoxFuture<'_, Result<Vec<EntryId>, SessionError>>;

    fn update_metadata_name(&self, name: Option<String>)
    -> BoxFuture<'_, Result<(), SessionError>>;

    fn update_metadata_model(
        &self,
        model: Option<String>,
    ) -> BoxFuture<'_, Result<(), SessionError>>;

    /// Delete the given entries and remove them from the children index.
    ///
    /// If the active cursor is among the deleted IDs it is reset to `None`.
    fn delete_entries(&self, ids: Vec<EntryId>) -> BoxFuture<'_, Result<(), SessionError>>;
}

// ── InMemorySessionStorage ─────────────────────────────────────────────────────

struct InMemoryState {
    metadata: SessionMetadata,
    entries: HashMap<EntryId, SessionEntry>,
    /// parent → list of child IDs (insertion order)
    children: HashMap<Option<EntryId>, Vec<EntryId>>,
}

/// In-memory session storage; useful for tests and ephemeral sessions.
pub struct InMemorySessionStorage {
    inner: Mutex<InMemoryState>,
}

impl InMemorySessionStorage {
    pub fn new(metadata: SessionMetadata) -> Self {
        Self {
            inner: Mutex::new(InMemoryState {
                metadata,
                entries: HashMap::new(),
                children: HashMap::new(),
            }),
        }
    }
}

impl SessionStorage for InMemorySessionStorage {
    fn metadata(&self) -> BoxFuture<'_, Result<SessionMetadata, SessionError>> {
        Box::pin(async move { Ok(self.inner.lock().unwrap().metadata.clone()) })
    }

    fn create_entry_id(&self) -> EntryId {
        EntryId::new()
    }

    fn append_entry(&self, entry: SessionEntry) -> BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            let mut st = self.inner.lock().unwrap();
            st.children
                .entry(entry.parent_id)
                .or_default()
                .push(entry.id);
            st.entries.insert(entry.id, entry);
            st.metadata.updated_at = chrono::Utc::now();
            Ok(())
        })
    }

    fn get_entry(&self, id: EntryId) -> BoxFuture<'_, Result<Option<SessionEntry>, SessionError>> {
        Box::pin(async move { Ok(self.inner.lock().unwrap().entries.get(&id).cloned()) })
    }

    fn children(&self, parent: EntryId) -> BoxFuture<'_, Result<Vec<SessionEntry>, SessionError>> {
        Box::pin(async move {
            let st = self.inner.lock().unwrap();
            let ids = st.children.get(&Some(parent)).cloned().unwrap_or_default();
            let result = ids
                .iter()
                .filter_map(|id| st.entries.get(id).cloned())
                .collect();
            Ok(result)
        })
    }

    fn all_leaves(&self) -> BoxFuture<'_, Result<Vec<EntryId>, SessionError>> {
        Box::pin(async move {
            let st = self.inner.lock().unwrap();
            // A leaf is an entry that has no children.
            let leaves = st
                .entries
                .keys()
                .filter(|id| {
                    st.children
                        .get(&Some(**id))
                        .map(|v| v.is_empty())
                        .unwrap_or(true)
                })
                .copied()
                .collect();
            Ok(leaves)
        })
    }

    fn active_cursor(&self) -> BoxFuture<'_, Result<Option<EntryId>, SessionError>> {
        Box::pin(async move { Ok(self.inner.lock().unwrap().metadata.active_cursor) })
    }

    fn set_active_cursor(&self, id: EntryId) -> BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            let mut st = self.inner.lock().unwrap();
            st.metadata.active_cursor = Some(id);
            st.metadata.updated_at = chrono::Utc::now();
            Ok(())
        })
    }

    fn path_to_root(
        &self,
        target: EntryId,
    ) -> BoxFuture<'_, Result<Vec<SessionEntry>, SessionError>> {
        Box::pin(async move {
            let st = self.inner.lock().unwrap();
            let mut path = Vec::new();
            let mut current = Some(target);
            while let Some(id) = current {
                match st.entries.get(&id) {
                    Some(entry) => {
                        current = entry.parent_id;
                        path.push(entry.clone());
                    }
                    None => return Err(SessionError::EntryNotFound(id)),
                }
            }
            path.reverse(); // root-first
            Ok(path)
        })
    }

    fn common_ancestor(
        &self,
        a: EntryId,
        b: EntryId,
    ) -> BoxFuture<'_, Result<Option<EntryId>, SessionError>> {
        Box::pin(async move {
            let st = self.inner.lock().unwrap();
            // Collect ancestor sets
            fn ancestors(
                entries: &HashMap<EntryId, SessionEntry>,
                start: EntryId,
            ) -> Result<Vec<EntryId>, SessionError> {
                let mut v = Vec::new();
                let mut cur = Some(start);
                while let Some(id) = cur {
                    v.push(id);
                    cur = entries
                        .get(&id)
                        .ok_or(SessionError::EntryNotFound(id))?
                        .parent_id;
                }
                Ok(v)
            }
            let a_anc = ancestors(&st.entries, a)?;
            let b_set: std::collections::HashSet<_> = {
                let b_anc = ancestors(&st.entries, b)?;
                b_anc.into_iter().collect()
            };
            Ok(a_anc.into_iter().find(|id| b_set.contains(id)))
        })
    }

    fn label_at(&self, id: EntryId) -> BoxFuture<'_, Result<Option<String>, SessionError>> {
        Box::pin(async move {
            use super::types::SessionEntryPayload;
            let st = self.inner.lock().unwrap();
            // Find a Label entry whose `from` target matches `id`.
            // For simplicity we scan all entries (acceptable for in-memory).
            for entry in st.entries.values() {
                if let SessionEntryPayload::Label { name } = &entry.payload
                    && entry.parent_id == Some(id)
                {
                    return Ok(Some(name.clone()));
                }
            }
            Ok(None)
        })
    }

    fn find_entries_by_type(
        &self,
        kind: SessionEntryKind,
    ) -> BoxFuture<'_, Result<Vec<EntryId>, SessionError>> {
        Box::pin(async move {
            let st = self.inner.lock().unwrap();
            let ids = st
                .entries
                .values()
                .filter(|e| e.kind() == kind)
                .map(|e| e.id)
                .collect();
            Ok(ids)
        })
    }

    fn update_metadata_name(
        &self,
        name: Option<String>,
    ) -> BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            self.inner.lock().unwrap().metadata.name = name;
            Ok(())
        })
    }

    fn update_metadata_model(
        &self,
        model: Option<String>,
    ) -> BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            self.inner.lock().unwrap().metadata.model = model;
            Ok(())
        })
    }

    fn delete_entries(&self, ids: Vec<EntryId>) -> BoxFuture<'_, Result<(), SessionError>> {
        Box::pin(async move {
            let ids_set: std::collections::HashSet<EntryId> = ids.into_iter().collect();
            let mut st = self.inner.lock().unwrap();
            for id in &ids_set {
                if let Some(entry) = st.entries.remove(id)
                    && let Some(children) = st.children.get_mut(&entry.parent_id)
                {
                    children.retain(|c| c != id);
                }
                st.children.remove(&Some(*id));
            }
            if let Some(cursor) = st.metadata.active_cursor
                && ids_set.contains(&cursor)
            {
                st.metadata.active_cursor = None;
            }
            Ok(())
        })
    }
}
