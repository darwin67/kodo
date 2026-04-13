use std::fs;

use kodo_ui::{
    command::Command, message::Message, model::Model, skills::load_skills, slash, update::update,
};
use tempfile::tempdir;

#[test]
fn fixture_skill_flows_from_discovery_to_injection() {
    let personal = tempdir().unwrap();
    let project = tempdir().unwrap();
    let skill_dir = project.path().join("skills/greet");
    fs::create_dir_all(skill_dir.join("scripts")).unwrap();
    fs::create_dir_all(skill_dir.join("references")).unwrap();
    fs::create_dir_all(skill_dir.join("assets")).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: greet
description: Greet someone by name
argument-hint: "[name]"
---
Hello, $ARGUMENTS! Welcome to kodo.

See [reference guide](references/guide.md) for details.
"#,
    )
    .unwrap();
    fs::write(skill_dir.join("scripts/helper.sh"), "#!/bin/sh").unwrap();
    fs::write(skill_dir.join("references/guide.md"), "Guide").unwrap();
    fs::write(skill_dir.join("assets/banner.txt"), "Banner").unwrap();

    let skills = load_skills(personal.path(), project.path());
    assert_eq!(skills.len(), 1);
    let skill = &skills[0];
    assert_eq!(skill.name, "greet");
    assert_eq!(skill.resources.scripts.len(), 1);
    assert_eq!(skill.resources.references.len(), 1);
    assert_eq!(skill.resources.assets.len(), 1);

    let commands = slash::merge_commands(skills);
    let completions = slash::complete("gr", &commands);
    assert_eq!(completions.len(), 1);
    assert_eq!(commands[completions[0]].name, "greet");

    let mut model = Model::new(false);
    model.commands = commands;
    model.input = "/greet Alice".to_string();
    model.cursor_pos = model.input.len();
    model.slash_state = slash::state_for_input(&model.input, &model.commands);

    let commands = update(&mut model, Message::SlashExecute);
    assert!(commands.iter().all(|command| command.is_none()));

    let pending = model.pending_skill_injection.as_deref().unwrap();
    assert!(pending.contains("Hello, Alice! Welcome to kodo."));
    assert!(pending.contains("<skill_resources"));
    assert!(pending.contains("scripts/helper.sh"));
    assert!(pending.contains("references/guide.md"));
    assert!(pending.contains("assets/banner.txt"));

    model.input = "Use that greeting.".to_string();
    let commands = update(&mut model, Message::Submit);
    assert!(model.pending_skill_injection.is_none());
    assert!(matches!(
        commands.as_slice(),
        [Command::SendToAgent(outbound)]
        if outbound.contains("Hello, Alice! Welcome to kodo.")
            && outbound.contains("<skill_resources")
            && outbound.ends_with("Use that greeting.")
    ));
}

#[test]
fn skill_without_resources_omits_manifest() {
    let personal = tempdir().unwrap();
    let project = tempdir().unwrap();
    let skill_dir = project.path().join("skills/plain");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: plain
description: Plain skill
---
Just do the thing for $ARGUMENTS.
"#,
    )
    .unwrap();

    let skills = load_skills(personal.path(), project.path());
    let mut model = Model::new(false);
    model.commands = slash::merge_commands(skills);
    model.input = "/plain task".to_string();
    model.cursor_pos = model.input.len();
    model.slash_state = slash::state_for_input(&model.input, &model.commands);

    update(&mut model, Message::SlashExecute);

    let pending = model.pending_skill_injection.as_deref().unwrap();
    assert!(pending.contains("Just do the thing for task."));
    assert!(!pending.contains("<skill_resources"));
}
