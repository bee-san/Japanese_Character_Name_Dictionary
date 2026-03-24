use std::{fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate should live inside the repository root")
        .to_path_buf()
}

fn read_repo_file(relative_path: &str) -> String {
    let path = repo_root().join(relative_path);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("failed to read {}: {}", path.display(), error);
    })
}

#[test]
fn honorific_skill_markdown_is_specific_and_complete() {
    let skill = read_repo_file("skills/add-honorifics/SKILL.md");

    for needle in [
        "name: add-honorifics",
        "description: Add or update Japanese honorific suffix support",
        "agents.md",
        "docs/plans/honorific_design.md",
        "HONORIFIC_SUFFIXES",
        "yomitan-dict-builder/src/name_parser.rs",
        "yomitan-dict-builder/src/dict_builder.rs",
        "yomitan-dict-builder/src/content_builder.rs",
        "cargo test honorific",
    ] {
        assert!(
            skill.contains(needle),
            "skill markdown should contain {needle:?}"
        );
    }

    assert!(
        !skill.contains("[TODO"),
        "skill markdown should not ship TODOs"
    );
    assert!(
        !skill.contains("Structuring This Skill"),
        "template guidance should be removed from the finished skill"
    );
}

#[test]
fn honorific_skill_ui_metadata_mentions_invocation() {
    let openai_yaml = read_repo_file("skills/add-honorifics/agents/openai.yaml");

    for needle in [
        "display_name: \"Add Honorifics\"",
        "short_description: \"Add or update honorific suffix support\"",
        "default_prompt: \"Use $add-honorifics",
    ] {
        assert!(
            openai_yaml.contains(needle),
            "agents/openai.yaml should contain {needle:?}"
        );
    }
}

#[test]
fn honorific_skill_is_listed_in_readme() {
    let readme = read_repo_file("README.md");

    assert!(
        readme.contains("## Codex Skills"),
        "README should have a Codex skills section"
    );
    assert!(
        readme.contains("`add-honorifics`"),
        "README should list the add-honorifics skill"
    );
}
