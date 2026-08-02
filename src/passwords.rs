// Password manager store

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPassword {
    pub id: String,
    pub domain: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PasswordStore {
    pub entries: Vec<SavedPassword>,
}

impl PasswordStore {
    pub fn add_password(&mut self, domain: String, username: String, password_plain: &str) -> String {
        let id = format!("pwd-{}", chrono::Utc::now().timestamp_millis());
        let created_at = chrono::Utc::now().to_rfc3339();
        
        // Simple obfuscation/hash for storage demonstration
        let password_hash = format!("enc_{}", password_plain);

        // Update if existing domain+username exists
        if let Some(existing) = self.entries.iter_mut().find(|e| e.domain == domain && e.username == username) {
            existing.password_hash = password_hash;
            return existing.id.clone();
        }

        self.entries.push(SavedPassword {
            id: id.clone(),
            domain,
            username,
            password_hash,
            created_at,
        });

        id
    }

    pub fn find_for_domain(&self, domain: &str) -> Vec<&SavedPassword> {
        self.entries.iter().filter(|e| e.domain.contains(domain)).collect()
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let initial_len = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() < initial_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_store() {
        let mut store = PasswordStore::default();
        let id = store.add_password("example.com".to_string(), "user1".to_string(), "secret123");
        assert_eq!(store.entries.len(), 1);

        let matches = store.find_for_domain("example.com");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].username, "user1");

        assert!(store.delete(&id));
        assert_eq!(store.entries.len(), 0);
    }
}
