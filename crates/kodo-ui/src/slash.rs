#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone)]
pub struct SlashState {
    pub completions: Vec<&'static SlashCommand>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlash {
    pub name: String,
    pub args: Vec<String>,
}

pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        args: "",
        description: "Print all available commands",
    },
    SlashCommand {
        name: "clear",
        args: "",
        description: "Clear conversation history",
    },
    SlashCommand {
        name: "compact",
        args: "",
        description: "Summarise context and replace messages",
    },
    SlashCommand {
        name: "model",
        args: "[id]",
        description: "Show current model, or switch to id",
    },
    SlashCommand {
        name: "providers",
        args: "",
        description: "List all connected accounts",
    },
    SlashCommand {
        name: "login",
        args: "<provider> [name]",
        description: "Add a new account",
    },
    SlashCommand {
        name: "logout",
        args: "<account_id>",
        description: "Remove an account from keychain + DB",
    },
];

pub fn complete(prefix: &str) -> Vec<&'static SlashCommand> {
    let prefix = prefix.trim().to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| {
            prefix.is_empty() || command.name.to_ascii_lowercase().starts_with(&prefix)
        })
        .collect()
}

pub fn parse(input: &str) -> ParsedSlash {
    let mut parts = input
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .map(ToOwned::to_owned);

    let name = parts.next().unwrap_or_default();
    let args = parts.collect();

    ParsedSlash { name, args }
}

pub fn is_slash_input(input: &str) -> bool {
    input.starts_with('/')
}

pub fn command_prefix(input: &str) -> &str {
    input
        .strip_prefix('/')
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default()
}

pub fn state_for_input(input: &str) -> Option<SlashState> {
    if !is_slash_input(input) {
        return None;
    }

    Some(SlashState {
        completions: complete(command_prefix(input)),
        selected: 0,
    })
}

pub fn format_help() -> String {
    let mut lines = vec!["Available slash commands:".to_string()];
    for command in COMMANDS {
        let signature = if command.args.is_empty() {
            format!("/{}", command.name)
        } else {
            format!("/{} {}", command.name, command.args)
        };
        lines.push(format!("{signature:<24} {}", command.description));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, complete, is_slash_input, parse};

    #[test]
    fn commands_contains_all_builtins() {
        assert_eq!(COMMANDS.len(), 7);
    }

    #[test]
    fn complete_with_empty_prefix_returns_all_commands() {
        assert_eq!(complete("").len(), COMMANDS.len());
    }

    #[test]
    fn complete_filters_model_command() {
        let matches = complete("mo");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "model");
    }

    #[test]
    fn complete_filters_login_and_logout() {
        let matches = complete("lo");
        let names: Vec<&str> = matches.into_iter().map(|command| command.name).collect();
        assert_eq!(names, vec!["login", "logout"]);
    }

    #[test]
    fn parse_model_command() {
        let parsed = parse("/model gpt-4o");
        assert_eq!(parsed.name, "model");
        assert_eq!(parsed.args, vec!["gpt-4o"]);
    }

    #[test]
    fn parse_login_command() {
        let parsed = parse("/login openai Work");
        assert_eq!(parsed.name, "login");
        assert_eq!(parsed.args, vec!["openai", "Work"]);
    }

    #[test]
    fn parse_help_command() {
        let parsed = parse("/help");
        assert_eq!(parsed.name, "help");
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn slash_input_detected_at_start() {
        assert!(is_slash_input("/m"));
        assert!(!is_slash_input("hello /no"));
    }
}
