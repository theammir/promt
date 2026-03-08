use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            timestamp: Utc::now(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            timestamp: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMetadata {
    pub id: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub metadata: ConversationMetadata,
    pub messages: Vec<Message>,
}

impl Conversation {
    pub fn new(provider: &str, model: &str) -> Self {
        let now = Utc::now();
        Self {
            metadata: ConversationMetadata {
                id: Uuid::new_v4().to_string(),
                created: now,
                updated: now,
                provider: provider.to_string(),
                model: model.to_string(),
                title: String::new(),
            },
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, message: Message) {
        // Auto-generate title from the first user message.
        if self.metadata.title.is_empty() && message.role == Role::User {
            self.metadata.title = message
                .content
                .chars()
                .take(60)
                .collect::<String>()
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
        }
        self.metadata.updated = Utc::now();
        self.messages.push(message);
    }

    /// Save conversation to its JSON file in the given directory.
    pub fn save(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let filename = format!(
            "{}_{}.json",
            self.metadata.created.format("%Y-%m-%d_%H-%M-%S"),
            &self.metadata.id[..8], // first 8 chars of UUID for stability
        );
        let path = dir.join(filename);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    /// Load a conversation from a JSON file.
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read_to_string(path)?;
        serde_json::from_str(&data).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// List all conversations in the given directory, sorted by modification time (newest first).
    pub fn list(dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        entries.sort_by(|a, b| {
            let ma = a.metadata().and_then(|m| m.modified()).ok();
            let mb = b.metadata().and_then(|m| m.modified()).ok();
            mb.cmp(&ma) // newest first
        });
        Ok(entries.into_iter().map(|e| e.path()).collect())
    }
}
