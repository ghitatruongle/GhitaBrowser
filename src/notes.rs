// Quick notes manager

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickNote {
    pub id: String,
    pub title: String,
    pub content: String,
    pub created_at: String,
    pub is_pinned: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoteStore {
    pub notes: Vec<QuickNote>,
    /// Monotonic counter so two notes created in the same millisecond still
    /// get distinct ids (timestamp-only ids collide and delete/pin would
    /// act on the wrong note).
    #[serde(default)]
    next_id: u64,
}

impl NoteStore {
    pub fn add_note(&mut self, title: String, content: String) -> String {
        self.next_id = self.next_id.wrapping_add(1);
        let id = format!(
            "note-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            self.next_id
        );
        let created_at = chrono::Utc::now().to_rfc3339();
        self.notes.push(QuickNote {
            id: id.clone(),
            title,
            content,
            created_at,
            is_pinned: false,
        });
        id
    }

    pub fn delete_note(&mut self, id: &str) -> bool {
        let initial_len = self.notes.len();
        self.notes.retain(|n| n.id != id);
        self.notes.len() < initial_len
    }

    pub fn toggle_pin(&mut self, id: &str) {
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == id) {
            note.is_pinned = !note.is_pinned;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_store() {
        let mut store = NoteStore::default();
        let id = store.add_note("Test Title".to_string(), "Test Content".to_string());
        assert_eq!(store.notes.len(), 1);

        store.toggle_pin(&id);
        assert!(store.notes[0].is_pinned);

        assert!(store.delete_note(&id));
        assert_eq!(store.notes.len(), 0);
    }
}
