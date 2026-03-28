use std::sync::Arc;

use anyhow::{Result, bail};
use clap::Parser;

use kodo_core::agent::Agent;
use kodo_core::mode::Mode;
use kodo_llm::anthropic::AnthropicProvider;
use kodo_llm::gemini::GeminiProvider;
use kodo_llm::ollama::OllamaProvider;
use kodo_llm::openai::OpenAiProvider;
use kodo_llm::provider::Provider;
use kodo_tui::terminal::read_user_input;

#[derive(Parser)]
#[command(name = "kodo", about = "A coding agent for your terminal")]
struct Cli {
    /// Model to use (e.g. claude-sonnet-4-20250514, gpt-4o, gemini-2.5-flash)
    #[arg(long, short)]
    model: Option<String>,

    /// Provider to use: anthropic, openai, gemini, ollama
    #[arg(long, short)]
    provider: Option<String>,
}

/// Create a provider by name.
fn create_provider(name: &str) -> Result<Arc<dyn Provider>> {
    // TODO
    // credentials should be able to be set on run time and stored.
    // and future processes/sessions will be able to reuse those crede
    match name {
        "anthropic" => Ok(Arc::new(AnthropicProvider::from_env()?)),
        "openai" => Ok(Arc::new(OpenAiProvider::from_env()?)),
        "gemini" => Ok(Arc::new(GeminiProvider::from_env()?)),
        "ollama" => Ok(Arc::new(OllamaProvider::from_env())),
        _ => bail!("unknown provider: {name}. Available: anthropic, openai, gemini, ollama"),
    }
}

/// Return the default model for a given provider name.
fn default_model(provider_name: &str) -> &str {
    match provider_name {
        "anthropic" => "claude-sonnet-4-20250514",
        "openai" => "gpt-4o",
        "gemini" => "gemini-2.5-flash",
        "ollama" => "llama3.1",
        _ => "claude-sonnet-4-20250514",
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (respects RUST_LOG env var).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    // Determine provider.
    let provider_name = cli.provider.as_deref().unwrap_or("anthropic");
    let provider = create_provider(provider_name)?;

    // Determine model.
    let model = cli
        .model
        .unwrap_or_else(|| default_model(provider_name).to_string());

    // Build the agent.
    let mut agent = Agent::new(provider).with_model(&model);

    // Register built-in tools.
    kodo_tools::register_builtin_tools(agent.tool_registry_mut());

    println!("kodo v{}", env!("CARGO_PKG_VERSION"));
    println!(
        "Provider: {} | Model: {}",
        agent.provider_name(),
        agent.model()
    );
    println!("Type your message and press Enter. Ctrl+D to exit.\n");

    // Main REPL loop.
    loop {
        let prompt = format!("[{}] > ", agent.mode);
        let input = match read_user_input(&prompt) {
            Ok(Some(input)) => input,
            Ok(None) => continue,
            Err(_) => break, // EOF or error
        };

        if input == "/quit" || input == "/exit" {
            break;
        }

        if input == "/mode" || input == "/mode plan" || input == "/mode build" {
            match input.as_str() {
                "/mode plan" => {
                    agent.mode = Mode::Plan;
                    println!("Switched to plan mode (read-only).\n");
                }
                "/mode build" => {
                    agent.mode = Mode::Build;
                    println!("Switched to build mode.\n");
                }
                _ => {
                    println!("Current mode: {}", agent.mode);
                    println!("  /mode plan   — read-only (search, read, web fetch)");
                    println!("  /mode build  — full execution (all tools)\n");
                }
            }
            continue;
        }

        if input.starts_with("/model") {
            let parts: Vec<&str> = input.splitn(3, ' ').collect();
            match parts.len() {
                1 => {
                    // /model — show current
                    println!(
                        "Provider: {} | Model: {}",
                        agent.provider_name(),
                        agent.model()
                    );
                    println!("  /model <name>              — switch model");
                    println!("  /model <provider> <name>   — switch provider and model");
                    println!("  Available providers: anthropic, openai, gemini, ollama\n");
                }
                2 => {
                    // /model <name> — switch model only
                    agent.set_model(parts[1]);
                    println!("Model set to: {}\n", agent.model());
                }
                3 => {
                    // /model <provider> <name> — switch provider + model
                    match create_provider(parts[1]) {
                        Ok(new_provider) => {
                            agent.set_provider(new_provider);
                            agent.set_model(parts[2]);
                            println!(
                                "Switched to {} / {} (conversation cleared)\n",
                                agent.provider_name(),
                                agent.model()
                            );
                        }
                        Err(e) => println!("Error: {e}\n"),
                    }
                }
                _ => unreachable!(),
            }
            continue;
        }

        if input == "/undo" {
            match agent.undo().await {
                Ok(msg) => println!("{msg}\n"),
                Err(e) => println!("Cannot undo: {e}\n"),
            }
            continue;
        }

        // TODO this will be replaced by modal operations on the TUI/GUI
        if input == "/tools" {
            let registry = agent.tool_registry();
            if registry.is_empty() {
                println!("No tools registered.");
            } else {
                println!("Registered tools ({}):\n", registry.len());
                let mut tools: Vec<_> = registry.iter().collect();
                tools.sort_by_key(|t| t.name());
                for tool in tools {
                    println!(
                        "  {:<20} [{:?}]  {}",
                        tool.name(),
                        tool.permission_level(),
                        tool.description()
                    );
                }
            }
            println!();
            continue;
        }

        if let Err(e) = agent.process_message(&input).await {
            eprintln!("\nerror: {e:#}");
        }

        println!();
    }

    let ctx = agent.context();
    println!(
        "\nSession stats: {} input tokens, {} output tokens",
        ctx.total_input_tokens, ctx.total_output_tokens
    );

    Ok(())
}
