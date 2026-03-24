use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use kodo_core::agent::Agent;
use kodo_llm::anthropic::AnthropicProvider;
use kodo_tui::terminal::read_user_input;

#[derive(Parser)]
#[command(name = "kodo", about = "A coding agent for your terminal")]
struct Cli {
    /// Model to use (e.g. claude-sonnet-4-20250514)
    #[arg(long, short)]
    model: Option<String>,
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

    // Initialize the Anthropic provider.
    let provider = Arc::new(AnthropicProvider::from_env()?);

    // Build the agent.
    let mut agent = Agent::new(provider);
    if let Some(model) = cli.model {
        agent = agent.with_model(model);
    }

    println!("kodo v{}", env!("CARGO_PKG_VERSION"));
    println!("Type your message and press Enter. Ctrl+D to exit.\n");

    // Main REPL loop.
    loop {
        let input = match read_user_input("> ") {
            Ok(Some(input)) => input,
            Ok(None) => continue,
            Err(_) => break, // EOF or error
        };

        if input == "/quit" || input == "/exit" {
            break;
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
