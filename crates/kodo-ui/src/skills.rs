use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillResources {
    pub scripts: Vec<PathBuf>,
    pub references: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
}

impl SkillResources {
    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty() && self.references.is_empty() && self.assets.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub disable_model_invocation: bool,
    pub user_invocable: bool,
    pub body: String,
    pub base_dir: PathBuf,
    pub resources: SkillResources,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "argument-hint")]
    argument_hint: Option<String>,
    #[serde(rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
    #[serde(rename = "user-invocable")]
    user_invocable: Option<bool>,
}

pub fn parse_skill_md(source: &str, fallback_name: &str, base_dir: PathBuf) -> Result<SkillDef> {
    let (frontmatter, body) = split_frontmatter(source)?;
    let body = body.trim().to_string();
    let description = frontmatter
        .description
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| first_paragraph(&body));

    Ok(SkillDef {
        name: frontmatter
            .name
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| fallback_name.to_string()),
        description,
        argument_hint: frontmatter
            .argument_hint
            .filter(|value| !value.trim().is_empty()),
        disable_model_invocation: frontmatter.disable_model_invocation.unwrap_or(false),
        user_invocable: frontmatter.user_invocable.unwrap_or(true),
        body,
        base_dir,
        resources: SkillResources::default(),
    })
}

pub fn default_skill_dirs() -> (PathBuf, PathBuf) {
    let personal = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".kodo");
    let project = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".kodo");
    (personal, project)
}

pub fn load_skills(personal_dir: &Path, project_dir: &Path) -> Vec<SkillDef> {
    let mut loaded = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    load_scope_skills(&mut loaded, &mut seen_names, &project_dir.join("skills"));
    load_scope_skills(&mut loaded, &mut seen_names, &personal_dir.join("skills"));
    load_legacy_commands(&mut loaded, &mut seen_names, &project_dir.join("commands"));

    loaded
}

pub fn enumerate_resources(base_dir: &Path) -> SkillResources {
    SkillResources {
        scripts: collect_direct_files(&base_dir.join("scripts")),
        references: collect_direct_files(&base_dir.join("references")),
        assets: collect_direct_files(&base_dir.join("assets")),
    }
}

pub fn render_body(body: &str, raw_args: &str) -> String {
    let tokens = tokenize_shell_args(raw_args);
    let mut rendered = String::new();
    let chars: Vec<char> = body.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '$' {
            rendered.push(chars[index]);
            index += 1;
            continue;
        }

        let remainder: String = chars[index + 1..].iter().collect();

        if let Some((digits, consumed)) = parse_arguments_index(&remainder) {
            let replacement = digits
                .parse::<usize>()
                .ok()
                .and_then(|arg_index| tokens.get(arg_index))
                .cloned()
                .unwrap_or_default();
            rendered.push_str(&replacement);
            index += 1 + consumed;
            continue;
        }

        if remainder.starts_with("ARGUMENTS") {
            rendered.push_str(raw_args);
            index += 1 + "ARGUMENTS".len();
            continue;
        }

        let digit_count = remainder
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .count();
        if digit_count > 0 {
            let replacement = remainder[..digit_count]
                .parse::<usize>()
                .ok()
                .and_then(|arg_index| tokens.get(arg_index))
                .cloned()
                .unwrap_or_default();
            rendered.push_str(&replacement);
            index += 1 + digit_count;
            continue;
        }

        rendered.push('$');
        index += 1;
    }

    if !body.contains("$ARGUMENTS") && !raw_args.trim().is_empty() {
        if !rendered.ends_with('\n') && !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("ARGUMENTS: ");
        rendered.push_str(raw_args);
    }

    rendered
}

pub fn format_resource_manifest(skill: &SkillDef) -> Option<String> {
    if skill.resources.is_empty() {
        return None;
    }

    let mut lines = vec![format!(
        "<skill_resources base_dir=\"{}\">",
        skill.base_dir.display()
    )];
    append_manifest_paths(
        &mut lines,
        "scripts",
        &skill.base_dir,
        &skill.resources.scripts,
    );
    append_manifest_paths(
        &mut lines,
        "references",
        &skill.base_dir,
        &skill.resources.references,
    );
    append_manifest_paths(
        &mut lines,
        "assets",
        &skill.base_dir,
        &skill.resources.assets,
    );
    lines.push("</skill_resources>".to_string());
    Some(lines.join("\n"))
}

fn split_frontmatter(source: &str) -> Result<(SkillFrontmatter, &str)> {
    if !source.starts_with("---") {
        return Ok((SkillFrontmatter::default(), source));
    }

    let remainder = &source[3..];
    let Some(rest) = remainder.strip_prefix('\n') else {
        bail!("invalid SKILL.md frontmatter");
    };
    let Some((yaml, body)) = rest.split_once("\n---\n") else {
        bail!("missing closing frontmatter delimiter");
    };
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(yaml).context("failed to parse SKILL.md frontmatter")?;
    Ok((frontmatter, body))
}

fn first_paragraph(body: &str) -> String {
    let paragraph = body
        .split("\n\n")
        .find(|part| !part.trim().is_empty())
        .map(|part| part.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "No description provided.".to_string());

    let mut truncated = paragraph.chars().take(250).collect::<String>();
    if paragraph.chars().count() > 250 {
        truncated.push_str("...");
    }
    truncated
}

fn load_scope_skills(
    loaded: &mut Vec<SkillDef>,
    seen_names: &mut std::collections::HashSet<String>,
    skills_dir: &Path,
) {
    let Ok(entries) = fs::read_dir(skills_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_file = path.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }

        match read_skill_file(&skill_file, path) {
            Ok(skill) => {
                if seen_names.insert(skill.name.clone()) {
                    loaded.push(skill);
                }
            }
            Err(error) => warn!(path = %skill_file.display(), "{error:#}"),
        }
    }
}

fn load_legacy_commands(
    loaded: &mut Vec<SkillDef>,
    seen_names: &mut std::collections::HashSet<String>,
    commands_dir: &Path,
) {
    let Ok(entries) = fs::read_dir(commands_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };

        if seen_names.contains(stem) {
            continue;
        }

        match read_skill_file(&path, commands_dir.to_path_buf()) {
            Ok(mut skill) => {
                skill.name = stem.to_string();
                if seen_names.insert(skill.name.clone()) {
                    loaded.push(skill);
                }
            }
            Err(error) => warn!(path = %path.display(), "{error:#}"),
        }
    }
}

fn read_skill_file(skill_file: &Path, base_dir: PathBuf) -> Result<SkillDef> {
    let source = fs::read_to_string(skill_file)
        .with_context(|| format!("failed to read {}", skill_file.display()))?;
    let fallback_name = base_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();
    let mut skill = parse_skill_md(&source, &fallback_name, base_dir)?;
    skill.resources = enumerate_resources(&skill.base_dir);
    Ok(skill)
}

fn collect_direct_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn tokenize_shell_args(raw_args: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = raw_args.chars().peekable();
    let mut quote = None::<char>;

    while let Some(ch) = chars.next() {
        match quote {
            Some(delimiter) if ch == delimiter => {
                quote = None;
            }
            Some(_) => {
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
            }
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None if ch == '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            None => current.push(ch),
        }
    }

    if !current.is_empty() || quote.is_some() {
        tokens.push(current);
    }

    tokens
}

fn parse_arguments_index(remainder: &str) -> Option<(&str, usize)> {
    let indexed = remainder.strip_prefix("ARGUMENTS[")?;
    let end = indexed.find(']')?;
    let digits = &indexed[..end];
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some((digits, "ARGUMENTS[".len() + end + 1))
}

fn append_manifest_paths(lines: &mut Vec<String>, tag: &str, base_dir: &Path, paths: &[PathBuf]) {
    for path in paths {
        let relative = path.strip_prefix(base_dir).unwrap_or(path);
        lines.push(format!("<{tag}>{}</{tag}>", relative.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn parse_skill_md_with_frontmatter() {
        let source = r#"---
name: greet
description: Friendly greeting
argument-hint: "[name]"
disable-model-invocation: true
user-invocable: false
---
Hello there.
"#;

        let base_dir = PathBuf::from("/tmp/greet");
        let skill = parse_skill_md(source, "fallback", base_dir.clone()).unwrap();

        assert_eq!(skill.name, "greet");
        assert_eq!(skill.description, "Friendly greeting");
        assert_eq!(skill.argument_hint.as_deref(), Some("[name]"));
        assert!(skill.disable_model_invocation);
        assert!(!skill.user_invocable);
        assert_eq!(skill.base_dir, base_dir);
    }

    #[test]
    fn parse_skill_md_without_frontmatter_uses_defaults() {
        let source = "First paragraph here.\n\nSecond paragraph.";
        let base_dir = PathBuf::from("/tmp/default");
        let skill = parse_skill_md(source, "fallback", base_dir.clone()).unwrap();

        assert_eq!(skill.name, "fallback");
        assert_eq!(skill.description, "First paragraph here.");
        assert!(!skill.disable_model_invocation);
        assert!(skill.user_invocable);
        assert_eq!(skill.base_dir, base_dir);
    }

    #[test]
    fn enumerate_resources_lists_direct_children() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("skill");
        fs::create_dir_all(base.join("scripts")).unwrap();
        fs::create_dir_all(base.join("references")).unwrap();
        fs::create_dir_all(base.join("assets")).unwrap();
        fs::write(base.join("scripts/run.sh"), "#!/bin/sh").unwrap();
        fs::write(base.join("references/guide.md"), "guide").unwrap();
        fs::write(base.join("assets/banner.txt"), "banner").unwrap();

        let resources = enumerate_resources(&base);

        assert_eq!(resources.scripts.len(), 1);
        assert_eq!(resources.references.len(), 1);
        assert_eq!(resources.assets.len(), 1);
        assert!(!resources.is_empty());
    }

    #[test]
    fn load_skills_uses_project_over_personal() {
        let personal = tempdir().unwrap();
        let project = tempdir().unwrap();
        let personal_skill = personal.path().join("skills/deploy");
        let project_skill = project.path().join("skills/deploy");
        fs::create_dir_all(&personal_skill).unwrap();
        fs::create_dir_all(&project_skill).unwrap();
        fs::write(personal_skill.join("SKILL.md"), "Personal deploy.").unwrap();
        fs::write(project_skill.join("SKILL.md"), "Project deploy.").unwrap();

        let skills = load_skills(personal.path(), project.path());

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "Project deploy.");
    }

    #[test]
    fn load_skills_picks_up_legacy_commands() {
        let personal = tempdir().unwrap();
        let project = tempdir().unwrap();
        fs::create_dir_all(project.path().join("commands")).unwrap();
        fs::write(project.path().join("commands/deploy.md"), "Deploy now.").unwrap();

        let skills = load_skills(personal.path(), project.path());

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "deploy");
    }

    #[test]
    fn render_body_substitutes_arguments() {
        let body = "Hello, $ARGUMENTS. First: $ARGUMENTS[0]. Second: $1.";
        let rendered = render_body(body, "\"hello world\" second");
        assert_eq!(
            rendered,
            "Hello, \"hello world\" second. First: hello world. Second: second."
        );
    }

    #[test]
    fn render_body_appends_arguments_when_placeholder_missing() {
        let rendered = render_body("Hello.", "Alice");
        assert_eq!(rendered, "Hello.\nARGUMENTS: Alice");
    }

    #[test]
    fn format_resource_manifest_omits_empty_skills() {
        let skill = SkillDef {
            name: "empty".to_string(),
            description: "desc".to_string(),
            argument_hint: None,
            disable_model_invocation: false,
            user_invocable: true,
            body: "body".to_string(),
            base_dir: PathBuf::from("/tmp/empty"),
            resources: SkillResources::default(),
        };

        assert!(format_resource_manifest(&skill).is_none());
    }
}
