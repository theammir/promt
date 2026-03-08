/// LLM provider abstraction using the `llm` crate.
///
/// Builds an LLMProvider from config, manages the active provider/model,
/// and converts our conversation messages to the llm crate's ChatMessage format.
use std::str::FromStr;
use std::sync::Arc;

use llm::builder::{LLMBackend, LLMBuilder};
use llm::chat::{ChatMessage, ChatRole};
use llm::LLMProvider;

use crate::config::Config;
use crate::conversation::{Message, Role};

/// Active provider state, including the built LLM client.
/// Uses Arc so the client can be shared with async streaming tasks.
pub struct ProviderState {
    pub provider_name: String,
    pub model_name: String,
    /// The active LLM client. None if building failed (e.g. no API key).
    pub client: Option<Arc<dyn LLMProvider>>,
    /// Error from last client build attempt, if any.
    pub build_error: Option<String>,
}

impl std::fmt::Debug for ProviderState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderState")
            .field("provider_name", &self.provider_name)
            .field("model_name", &self.model_name)
            .field("has_client", &self.client.is_some())
            .field("build_error", &self.build_error)
            .finish()
    }
}

impl ProviderState {
    /// Build provider state from config. Attempts to create an LLM client.
    pub fn from_config(config: &Config) -> Self {
        let provider_name = config.general.default_provider.clone();
        let model_name = config.general.default_model.clone();

        let (client, build_error) = build_client(config, &provider_name, &model_name);

        Self {
            provider_name,
            model_name,
            client,
            build_error,
        }
    }

    /// Switch to a different provider/model. Rebuilds the LLM client.
    pub fn switch(&mut self, config: &Config, provider: &str, model: &str) {
        self.provider_name = provider.to_string();
        self.model_name = model.to_string();
        let (client, build_error) = build_client(config, provider, model);
        self.client = client;
        self.build_error = build_error;
    }
}

/// Build an LLM client from config for the given provider/model.
fn build_client(
    config: &Config,
    provider_name: &str,
    model_name: &str,
) -> (Option<Arc<dyn LLMProvider>>, Option<String>) {
    build_client_with_system(config, provider_name, model_name, None)
}

/// Build an LLM client with an optional system prompt.
pub fn build_client_with_system(
    config: &Config,
    provider_name: &str,
    model_name: &str,
    system_prompt: Option<&str>,
) -> (Option<Arc<dyn LLMProvider>>, Option<String>) {
    let backend = match LLMBackend::from_str(provider_name) {
        Ok(b) => b,
        Err(e) => return (None, Some(format!("Unknown provider: {e}"))),
    };

    let mut builder = LLMBuilder::new().backend(backend).model(model_name);

    // Set API key if available.
    if let Some(key) = config.api_key(provider_name) {
        builder = builder.api_key(key);
    }

    // Set base URL if available (important for Ollama and custom endpoints).
    if let Some(url) = config.base_url(provider_name) {
        builder = builder.base_url(url);
    }

    // Reasonable defaults.
    builder = builder.max_tokens(4096).timeout_seconds(120);

    // Set system prompt if provided.
    if let Some(prompt) = system_prompt {
        builder = builder.system(prompt);
    }

    match builder.build() {
        Ok(client) => (Some(Arc::from(client)), None),
        Err(e) => (None, Some(format!("{e}"))),
    }
}

/// Convert our conversation messages into the llm crate's ChatMessage format.
pub fn to_chat_messages(messages: &[Message]) -> Vec<ChatMessage> {
    messages
        .iter()
        .filter_map(|msg| {
            let role = match msg.role {
                Role::User => ChatRole::User,
                Role::Assistant => ChatRole::Assistant,
                // The llm crate doesn't have a System role in ChatMessage;
                // system prompts are set via the builder. Skip system messages here.
                Role::System => return None,
            };
            Some(match role {
                ChatRole::User => ChatMessage::user().content(&msg.content).build(),
                ChatRole::Assistant => ChatMessage::assistant().content(&msg.content).build(),
            })
        })
        .collect()
}

/// Known provider/model pairs for the model picker.
/// These are common defaults; runtime model listing can supplement these.
pub fn known_models() -> Vec<(String, String)> {
    vec![
        // OpenAI
        ("openai".into(), "gpt-4.1".into()),
        ("openai".into(), "gpt-4.1-mini".into()),
        ("openai".into(), "gpt-4.1-nano".into()),
        ("openai".into(), "o4-mini".into()),
        ("openai".into(), "o3".into()),
        ("openai".into(), "o3-mini".into()),
        // Anthropic
        ("anthropic".into(), "claude-sonnet-4-20250514".into()),
        ("anthropic".into(), "claude-opus-4-20250514".into()),
        ("anthropic".into(), "claude-haiku-3-20250414".into()),
        // Google
        ("google".into(), "gemini-2.5-pro".into()),
        ("google".into(), "gemini-2.5-flash".into()),
        ("google".into(), "gemini-2.0-flash".into()),
        // DeepSeek
        ("deepseek".into(), "deepseek-chat".into()),
        ("deepseek".into(), "deepseek-reasoner".into()),
        // Ollama (user must have these locally)
        ("ollama".into(), "llama3".into()),
        ("ollama".into(), "mistral".into()),
        ("ollama".into(), "codellama".into()),
        // Groq
        ("groq".into(), "llama-3.3-70b-versatile".into()),
        ("groq".into(), "mixtral-8x7b-32768".into()),
        // XAI
        ("xai".into(), "grok-3".into()),
        ("xai".into(), "grok-3-mini".into()),
        // OpenRouter
        ("openrouter".into(), "anthropic/claude-sonnet-4".into()),
        ("openrouter".into(), "openai/gpt-4.1".into()),
        // Mistral
        ("mistral".into(), "mistral-large-latest".into()),
        ("mistral".into(), "mistral-small-latest".into()),
        // Cohere
        ("cohere".into(), "command-r-plus".into()),
        ("cohere".into(), "command-r".into()),
    ]
}
