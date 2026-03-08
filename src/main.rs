mod app;
mod clipboard;
mod config;
mod conversation;
mod highlight;
mod keymap;
mod mode;
mod provider;
mod ui;

use std::io::{self, IsTerminal, Read};

use clap::{Parser, Subcommand};

use crate::app::App;
use crate::config::Config;
use crate::conversation::Conversation;

#[derive(Parser)]
#[command(name = "promt", version, about = "Inline TUI for LLM prompting")]
struct Cli {
    /// Send a one-shot prompt (non-interactive).
    prompt: Option<String>,

    /// Override provider.
    #[arg(short, long)]
    provider: Option<String>,

    /// Override model.
    #[arg(short, long)]
    model: Option<String>,

    /// Continue last conversation.
    #[arg(short, long)]
    r#continue: bool,

    /// Set system prompt.
    #[arg(short, long)]
    system: Option<String>,

    /// Disable streaming (wait for full response).
    #[arg(long)]
    no_stream: bool,

    /// Raw text output, no TUI (for piping).
    #[arg(long)]
    raw: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Browse past conversations.
    History,
    /// Print or open config.
    Config,
    /// List configured providers and their models.
    Providers,
}

fn main() {
    let cli = Cli::parse();

    // Load config.
    let mut config = Config::load();

    // First-run setup: create config file with defaults if it doesn't exist.
    if let Some(path) = Config::path() {
        if !path.exists() {
            if let Err(e) = config.save() {
                eprintln!("Note: could not create default config: {e}");
            } else {
                eprintln!("Created default config at {}", path.display());
            }
        }
    }

    // Apply CLI overrides.
    if let Some(ref p) = cli.provider {
        config.general.default_provider = p.clone();
    }
    if let Some(ref m) = cli.model {
        config.general.default_model = m.clone();
    }

    match cli.command {
        Some(Commands::History) => {
            cmd_history();
        }
        Some(Commands::Config) => {
            if let Some(path) = Config::path() {
                println!("{}", path.display());
            }
        }
        Some(Commands::Providers) => {
            cmd_providers(&config);
        }
        None => {
            if cli.raw || cli.prompt.is_some() || !io::stdin().is_terminal() {
                // Non-interactive / pipe mode.
                let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                rt.block_on(async {
                    run_non_interactive(&cli, &config).await;
                });
                return;
            }

            // Interactive TUI mode.
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                let mut app = App::new(config.clone());

                // Apply --system flag.
                if let Some(ref system) = cli.system {
                    app.system_prompt = Some(system.clone());
                    app.show_system_prompt = true;
                    app.rebuild_client();
                }

                // Apply --continue flag: load the most recent conversation.
                if cli.r#continue {
                    if let Some(dir) = Config::conversations_dir() {
                        match Conversation::list(&dir) {
                            Ok(paths) if !paths.is_empty() => {
                                match Conversation::load(&paths[0]) {
                                    Ok(conv) => {
                                        app.conversation = conv;
                                        app.message_list.render_all(
                                            &app.conversation.messages,
                                            app.conceal,
                                            &app.highlighter,
                                        );
                                        app.needs_scroll_bottom = true;
                                        app.status.status_message =
                                            Some("Continued previous conversation".to_string());
                                        app.set_status_timer();
                                    }
                                    Err(e) => {
                                        app.status.status_message = Some(format!(
                                            "Could not load last conversation: {e}"
                                        ));
                                        app.set_status_timer();
                                    }
                                }
                            }
                            Ok(paths) if paths.is_empty() => {
                                app.status.status_message =
                                    Some("No previous conversations found".to_string());
                                app.set_status_timer();
                            }
                            Err(e) => {
                                app.status.status_message =
                                    Some(format!("Error listing conversations: {e}"));
                                app.set_status_timer();
                            }
                            _ => {
                                // Ok(paths) with empty list already handled above
                            }
                        }
                    }
                }

                if let Err(e) = app.run().await {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            });
        }
    }
}

/// `promt history` - list saved conversations.
fn cmd_history() {
    let Some(dir) = Config::conversations_dir() else {
        eprintln!("Cannot determine data directory.");
        std::process::exit(1);
    };
    match Conversation::list(&dir) {
        Ok(paths) => {
            if paths.is_empty() {
                println!("No saved conversations.");
                return;
            }
            for path in &paths {
                match Conversation::load(path) {
                    Ok(conv) => {
                        let title = if conv.metadata.title.is_empty() {
                            "(untitled)"
                        } else {
                            &conv.metadata.title
                        };
                        let date = conv.metadata.updated.format("%Y-%m-%d %H:%M");
                        let msgs = conv.messages.len();
                        println!(
                            "{date}  {msgs:>3} msgs  {}/{} -- {title}",
                            conv.metadata.provider, conv.metadata.model
                        );
                    }
                    Err(_) => {
                        if let Some(name) = path.file_stem() {
                            println!("  (unreadable) {}", name.to_string_lossy());
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error listing conversations: {e}");
            std::process::exit(1);
        }
    }
}

/// `promt providers` - list known providers and models.
fn cmd_providers(config: &Config) {
    println!("Known providers and models:");
    println!();

    let models = provider::known_models();
    let mut current_provider = String::new();

    for (prov, model) in &models {
        if prov != &current_provider {
            if !current_provider.is_empty() {
                println!();
            }
            let has_key = config.api_key(prov).is_some();
            let status = if has_key { "(configured)" } else { "" };
            println!("  {prov} {status}");
            current_provider = prov.clone();
        }
        println!("    {model}");
    }

    // Show configured favorites.
    let favs = config.favorites_ordered();
    if !favs.is_empty() {
        println!();
        println!("Favorites:");
        for (n, fav) in &favs {
            println!("  {n}: {}/{}", fav.provider, fav.model);
        }
    }
}

/// Non-interactive mode: send prompt, stream response to stdout.
async fn run_non_interactive(cli: &Cli, config: &Config) {
    use futures::StreamExt;

    // Gather the prompt text.
    let mut prompt_text = String::new();

    // Read from stdin if piped.
    if !io::stdin().is_terminal() {
        if let Err(e) = io::stdin().read_to_string(&mut prompt_text) {
            eprintln!("Failed to read stdin: {e}");
            std::process::exit(1);
        }
    }

    // Append CLI positional prompt if provided.
    if let Some(ref p) = cli.prompt {
        if !prompt_text.is_empty() {
            prompt_text.push('\n');
        }
        prompt_text.push_str(p);
    }

    if prompt_text.trim().is_empty() {
        eprintln!("No prompt provided.");
        std::process::exit(1);
    }

    // Build client.
    let system_prompt = cli.system.as_deref();
    let (client, build_error) = provider::build_client_with_system(
        config,
        &config.general.default_provider,
        &config.general.default_model,
        system_prompt,
    );

    let Some(client) = client else {
        let err = build_error.unwrap_or_else(|| "Unknown error".to_string());
        eprintln!("Failed to create LLM client: {err}");
        std::process::exit(1);
    };

    // Build messages.
    let messages = vec![
        llm::chat::ChatMessage::user()
            .content(&prompt_text)
            .build(),
    ];

    if cli.no_stream {
        // Non-streaming: get full response.
        match client.chat(&messages).await {
            Ok(response) => {
                print!("{}", response.text().unwrap_or_default());
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        // Streaming to stdout.
        match client.chat_stream(&messages).await {
            Ok(mut stream) => {
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(token) => {
                            print!("{token}");
                        }
                        Err(e) => {
                            eprintln!("\nError during streaming: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                println!();
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}
