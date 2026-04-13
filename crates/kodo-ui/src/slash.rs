use crate::skills::SkillDef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    Skill(SkillDef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub args: String,
    pub description: String,
    pub source: CommandSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashState {
    pub completions: Vec<usize>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlash {
    pub name: String,
    pub args: Vec<String>,
    pub args_raw: String,
}

impl SlashCommand {
    pub fn signature(&self) -> String {
        if self.args.is_empty() {
            format!("/{}", self.name)
        } else {
            format!("/{} {}", self.name, self.args)
        }
    }

    pub fn is_user_invocable(&self) -> bool {
        match &self.source {
            CommandSource::Builtin => true,
            CommandSource::Skill(skill) => skill.user_invocable,
        }
    }

    pub fn is_manual_only(&self) -> bool {
        matches!(
            &self.source,
            CommandSource::Skill(SkillDef {
                disable_model_invocation: true,
                ..
            })
        )
    }
}

pub fn builtin_commands() -> Vec<SlashCommand> {
    [
        ("help", "", "Print all available commands"),
        ("clear", "", "Clear conversation history"),
        ("compact", "", "Summarise context and replace messages"),
        ("model", "[id]", "Show current model, or switch to id"),
        ("providers", "", "List all connected accounts"),
        ("login", "<provider> [name]", "Add a new account"),
        (
            "logout",
            "<account_id>",
            "Remove an account from keychain + DB",
        ),
    ]
    .into_iter()
    .map(|(name, args, description)| SlashCommand {
        name: name.to_string(),
        args: args.to_string(),
        description: description.to_string(),
        source: CommandSource::Builtin,
    })
    .collect()
}

pub fn merge_commands(skills: Vec<SkillDef>) -> Vec<SlashCommand> {
    let mut commands = builtin_commands();
    commands.extend(skills.into_iter().map(|skill| SlashCommand {
        name: skill.name.clone(),
        args: skill.argument_hint.clone().unwrap_or_default(),
        description: skill.description.clone(),
        source: CommandSource::Skill(skill),
    }));
    commands
}

pub fn complete(prefix: &str, commands: &[SlashCommand]) -> Vec<usize> {
    let prefix = prefix.trim().to_ascii_lowercase();
    commands
        .iter()
        .enumerate()
        .filter(|(_, command)| {
            command.is_user_invocable()
                && (prefix.is_empty() || command.name.to_ascii_lowercase().starts_with(&prefix))
        })
        .map(|(index, _)| index)
        .collect()
}

pub fn parse(input: &str) -> ParsedSlash {
    let trimmed = input.trim().trim_start_matches('/');
    let mut parts = trimmed.split_whitespace().map(ToOwned::to_owned);

    let name = parts.next().unwrap_or_default();
    let args = parts.collect::<Vec<_>>();
    let args_raw = trimmed
        .strip_prefix(&name)
        .unwrap_or_default()
        .trim()
        .to_string();

    ParsedSlash {
        name,
        args,
        args_raw,
    }
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

pub fn state_for_input(input: &str, commands: &[SlashCommand]) -> Option<SlashState> {
    if !is_slash_input(input) {
        return None;
    }

    Some(SlashState {
        completions: complete(command_prefix(input), commands),
        selected: 0,
    })
}

pub fn format_help(commands: &[SlashCommand]) -> String {
    let mut lines = vec!["Available slash commands:".to_string()];

    for command in commands
        .iter()
        .filter(|command| command.is_user_invocable())
    {
        let mut description = command.description.clone();
        if command.is_manual_only() {
            description.push_str(" [manual]");
        }
        lines.push(format!("{:<24} {}", command.signature(), description));
    }

    lines.join("\n")
}

pub fn find_user_command<'a>(commands: &'a [SlashCommand], name: &str) -> Option<&'a SlashCommand> {
    commands
        .iter()
        .find(|command| command.is_user_invocable() && command.name == name)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::skills::{SkillDef, SkillResources};

    use super::{
        CommandSource, builtin_commands, complete, format_help, is_slash_input, merge_commands,
        parse,
    };

    #[test]
    fn commands_contains_all_builtins() {
        assert_eq!(builtin_commands().len(), 7);
    }

    #[test]
    fn complete_with_empty_prefix_returns_all_builtins() {
        let commands = builtin_commands();
        assert_eq!(complete("", &commands).len(), commands.len());
    }

    #[test]
    fn complete_filters_model_command() {
        let commands = builtin_commands();
        let matches = complete("mo", &commands);
        assert_eq!(matches.len(), 1);
        assert_eq!(commands[matches[0]].name, "model");
    }

    #[test]
    fn complete_filters_login_and_logout() {
        let commands = builtin_commands();
        let matches = complete("lo", &commands);
        let names: Vec<&str> = matches
            .into_iter()
            .map(|index| commands[index].name.as_str())
            .collect();
        assert_eq!(names, vec!["login", "logout"]);
    }

    #[test]
    fn complete_omits_non_invocable_skills() {
        let commands = merge_commands(vec![SkillDef {
            name: "hidden".to_string(),
            description: "Hidden skill".to_string(),
            argument_hint: None,
            disable_model_invocation: false,
            user_invocable: false,
            body: "body".to_string(),
            base_dir: PathBuf::from("/tmp/hidden"),
            resources: SkillResources::default(),
        }]);

        let matches = complete("", &commands);
        assert!(
            !matches
                .into_iter()
                .any(|index| commands[index].name == "hidden")
        );
    }

    #[test]
    fn help_marks_manual_only_skills() {
        let commands = merge_commands(vec![SkillDef {
            name: "greet".to_string(),
            description: "Greet somebody".to_string(),
            argument_hint: Some("[name]".to_string()),
            disable_model_invocation: true,
            user_invocable: true,
            body: "body".to_string(),
            base_dir: PathBuf::from("/tmp/greet"),
            resources: SkillResources::default(),
        }]);

        let help = format_help(&commands);
        assert!(help.contains("/greet [name]"));
        assert!(help.contains("[manual]"));
    }

    #[test]
    fn parse_model_command() {
        let parsed = parse("/model gpt-4o");
        assert_eq!(parsed.name, "model");
        assert_eq!(parsed.args, vec!["gpt-4o"]);
        assert_eq!(parsed.args_raw, "gpt-4o");
    }

    #[test]
    fn parse_login_command() {
        let parsed = parse("/login openai Work");
        assert_eq!(parsed.name, "login");
        assert_eq!(parsed.args, vec!["openai", "Work"]);
        assert_eq!(parsed.args_raw, "openai Work");
    }

    #[test]
    fn slash_input_detected_at_start() {
        assert!(is_slash_input("/m"));
        assert!(!is_slash_input("hello /no"));
    }

    #[test]
    fn merge_commands_wraps_skills() {
        let commands = merge_commands(vec![SkillDef {
            name: "deploy".to_string(),
            description: "Deploy the app".to_string(),
            argument_hint: Some("[env]".to_string()),
            disable_model_invocation: false,
            user_invocable: true,
            body: "body".to_string(),
            base_dir: PathBuf::from("/tmp/deploy"),
            resources: SkillResources::default(),
        }]);

        assert!(matches!(
            commands.last().map(|command| &command.source),
            Some(CommandSource::Skill(_))
        ));
    }
}
