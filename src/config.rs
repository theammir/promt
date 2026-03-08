use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level application config, loaded from ~/.config/promt/config.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub favorites: HashMap<String, FavoriteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_provider")]
    pub default_provider: String,
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default = "default_conceal_level")]
    pub conceal_level: u8,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_leader_key")]
    pub leader_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteEntry {
    pub provider: String,
    pub model: String,
}

// Defaults

fn default_provider() -> String {
    "openai".to_string()
}

fn default_model() -> String {
    "gpt-4.1-nano".to_string()
}

fn default_conceal_level() -> u8 {
    1
}

fn default_theme() -> String {
    "base16-ocean.dark".to_string()
}

fn default_leader_key() -> String {
    "C-x".to_string()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_provider: default_provider(),
            default_model: default_model(),
            conceal_level: default_conceal_level(),
            theme: default_theme(),
            leader_key: default_leader_key(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            providers: HashMap::new(),
            favorites: HashMap::new(),
        }
    }
}

impl Config {
    /// Standard config file path: ~/.config/promt/config.toml
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("promt").join("config.toml"))
    }

    /// Standard data directory: ~/.local/share/promt/
    pub fn data_dir() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("promt"))
    }

    /// Conversations directory: ~/.local/share/promt/conversations/
    pub fn conversations_dir() -> Option<PathBuf> {
        Self::data_dir().map(|d| d.join("conversations"))
    }

    /// Load config from disk. Returns default config if the file doesn't exist.
    /// Prints a warning to stderr if the config file exists but is malformed.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match Self::load_from(&path) {
            Ok(config) => config,
            Err(ConfigError::Io(_)) => {
                // File doesn't exist or isn't readable — use defaults silently.
                Self::default()
            }
            Err(e @ (ConfigError::Parse(_) | ConfigError::Serialize(_))) => {
                eprintln!(
                    "Warning: config at {} is malformed, using defaults: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Load from a specific path.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let mut config: Config = toml::from_str(&content).map_err(ConfigError::Parse)?;
        config.apply_env_overrides();
        Ok(config)
    }

    /// Save config to its standard path.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::path().ok_or_else(|| {
            ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not determine config directory",
            ))
        })?;
        self.save_to(&path)
    }

    /// Save to a specific path.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Io)?;
        }
        let content = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        std::fs::write(path, content).map_err(ConfigError::Io)
    }

    /// Override provider API keys from environment variables.
    /// Checks: OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY, etc.
    fn apply_env_overrides(&mut self) {
        let env_mappings = [
            ("openai", "OPENAI_API_KEY"),
            ("anthropic", "ANTHROPIC_API_KEY"),
            ("google", "GOOGLE_API_KEY"),
            ("deepseek", "DEEPSEEK_API_KEY"),
            ("xai", "XAI_API_KEY"),
            ("groq", "GROQ_API_KEY"),
            ("cohere", "COHERE_API_KEY"),
            ("mistral", "MISTRAL_API_KEY"),
            ("huggingface", "HUGGINGFACE_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
            ("phind", "PHIND_API_KEY"),
        ];

        for (provider, env_var) in env_mappings {
            if let Ok(key) = std::env::var(env_var) {
                self.providers
                    .entry(provider.to_string())
                    .or_insert_with(|| ProviderConfig {
                        api_key: None,
                        base_url: None,
                    })
                    .api_key = Some(key);
            }
        }

        // Ollama base URL override
        if let Ok(url) = std::env::var("OLLAMA_HOST") {
            self.providers
                .entry("ollama".to_string())
                .or_insert_with(|| ProviderConfig {
                    api_key: None,
                    base_url: None,
                })
                .base_url = Some(url);
        }
    }

    /// Get the API key for a provider, with env var fallback already applied.
    pub fn api_key(&self, provider: &str) -> Option<&str> {
        self.providers
            .get(provider)
            .and_then(|p| p.api_key.as_deref())
    }

    /// Get the base URL for a provider, if configured.
    pub fn base_url(&self, provider: &str) -> Option<&str> {
        self.providers
            .get(provider)
            .and_then(|p| p.base_url.as_deref())
    }

    /// Get a favorite entry by number (1-9).
    pub fn favorite(&self, n: u8) -> Option<&FavoriteEntry> {
        self.favorites.get(&n.to_string())
    }

    /// Get an ordered list of favorites for cycling.
    pub fn favorites_ordered(&self) -> Vec<(u8, &FavoriteEntry)> {
        let mut favs: Vec<_> = self
            .favorites
            .iter()
            .filter_map(|(k, v)| k.parse::<u8>().ok().map(|n| (n, v)))
            .collect();
        favs.sort_by_key(|(k, _)| *k);
        favs
    }
}

// We need toml for serialization/deserialization, add a small wrapper
// since we're using the `toml` crate.
mod toml {
    pub fn from_str<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, TomlError> {
        ::toml::from_str(s).map_err(TomlError)
    }

    pub fn to_string_pretty<T: serde::Serialize>(value: &T) -> Result<String, TomlSerError> {
        ::toml::to_string_pretty(value).map_err(TomlSerError)
    }

    #[derive(Debug)]
    pub struct TomlError(pub ::toml::de::Error);

    #[derive(Debug)]
    pub struct TomlSerError(pub ::toml::ser::Error);
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::TomlError),
    Serialize(toml::TomlSerError),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config I/O error: {e}"),
            ConfigError::Parse(e) => write!(f, "config parse error: {}", e.0),
            ConfigError::Serialize(e) => write!(f, "config serialize error: {}", e.0),
        }
    }
}

impl std::error::Error for ConfigError {}
