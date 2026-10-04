use super::{BuildSystemPromptOptions, ContextFile, SkillMetadata, build_system_prompt};
use std::collections::HashMap;

fn options() -> BuildSystemPromptOptions {
    BuildSystemPromptOptions {
        cwd: "/work/project".into(),
        ..Default::default()
    }
}

fn skill(name: &str) -> SkillMetadata {
    SkillMetadata {
        name: name.into(),
        description: Some("specialized help".into()),
        disable_model_invocation: false,
        path: "/skills/demo/SKILL.md".into(),
    }
}

#[test]
fn empty_input_uses_default_prompt() {
    let prompt = build_system_prompt(options());
    assert_eq!(
        prompt,
        "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\nAvailable tools:\n(none)\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n\nGuidelines:\n- Use bash for file operations like ls, rg, find\n- Be concise in your responses\n- Show file paths clearly when working with files\nCurrent working directory: /work/project"
    );
}

#[test]
fn custom_prompt_still_appends_context_skills_and_cwd() {
    let mut options = options();
    options.custom_prompt = Some("Custom instructions".into());
    options.append_system_prompt = Some("Extra instructions".into());
    options.context_files = vec![ContextFile {
        path: "root/AGENTS.md".into(),
        content: "root rules".into(),
    }];
    options.selected_tools = Some(vec!["read".into()]);
    options.skills = vec![skill("demo")];

    let prompt = build_system_prompt(options);
    assert!(prompt.starts_with("Custom instructions\n\nExtra instructions"));
    assert!(prompt.contains("<project_context>\n\nProject-specific instructions and guidelines:"));
    assert!(prompt.contains(
        "<project_instructions path=\"root/AGENTS.md\">\nroot rules\n</project_instructions>"
    ));
    assert!(prompt.contains("<available_skills>"));
    assert!(prompt.ends_with("\nCurrent working directory: /work/project\n"));
}

#[test]
fn selected_tools_filter_snippets_and_guidelines() {
    let mut options = options();
    options.selected_tools = Some(vec!["bash".into(), "read".into(), "edit".into()]);
    options.tool_snippets = HashMap::from([
        ("bash".into(), "run commands".into()),
        ("read".into(), "read files".into()),
        ("write".into(), "write files".into()),
    ]);
    options.prompt_guidelines = vec![
        "  Keep paths visible  ".into(),
        "Keep paths visible".into(),
        "  ".into(),
    ];

    let prompt = build_system_prompt(options);
    assert!(prompt.contains("- bash: run commands\n- read: read files"));
    assert!(!prompt.contains("- write: write files"));
    assert!(prompt.contains("- Use bash for file operations like ls, rg, find"));
    assert_eq!(prompt.matches("- Keep paths visible").count(), 1);
    assert!(!prompt.contains("-   "));
}

#[test]
fn empty_tool_snippets_are_omitted() {
    let mut options = options();
    options.selected_tools = Some(vec!["read".into()]);
    options.tool_snippets = HashMap::from([(String::from("read"), String::new())]);

    let prompt = build_system_prompt(options);

    assert!(!prompt.contains("- read:"));
}

#[test]
fn duplicate_guidelines_are_trimmed_and_deduplicated() {
    let mut options = options();
    options.prompt_guidelines = vec![
        "  same rule ".into(),
        "same rule".into(),
        "another rule".into(),
    ];

    let prompt = build_system_prompt(options);
    assert_eq!(prompt.matches("- same rule").count(), 1);
    assert!(prompt.contains("- another rule"));
    assert!(prompt.contains("- Be concise in your responses"));
    assert!(prompt.contains("- Show file paths clearly when working with files"));
}

#[test]
fn project_context_keeps_input_order_and_xml_wrappers() {
    let mut options = options();
    options.custom_prompt = Some("base".into());
    options.context_files = vec![
        ContextFile {
            path: "root/AGENTS.md".into(),
            content: "root".into(),
        },
        ContextFile {
            path: "src/AGENTS.md".into(),
            content: "near".into(),
        },
    ];

    let prompt = build_system_prompt(options);
    let root = prompt.find("path=\"root/AGENTS.md\"").unwrap();
    let near = prompt.find("path=\"src/AGENTS.md\"").unwrap();
    assert!(root < near);
    assert!(
        prompt.contains("<project_context>\n\nProject-specific instructions and guidelines:\n\n")
    );
    assert!(prompt.ends_with("</project_context>\n\nCurrent working directory: /work/project\n"));
}

#[test]
fn skills_are_omitted_without_a_read_capable_tool() {
    let mut options = options();
    options.selected_tools = Some(vec!["edit".into(), "write".into()]);
    options.skills = vec![skill("hidden")];

    assert!(!build_system_prompt(options).contains("hidden"));
}

#[test]
fn disabled_skills_are_omitted_from_prompt() {
    let mut options = options();
    options.selected_tools = Some(vec!["read".into()]);
    let mut disabled = skill("disabled");
    disabled.disable_model_invocation = true;
    options.skills = vec![disabled];

    assert!(!build_system_prompt(options).contains("disabled"));
}

#[test]
fn skills_are_included_with_read_and_bash() {
    for tool in ["read", "bash"] {
        let mut options = options();
        options.selected_tools = Some(vec![tool.into()]);
        options.skills = vec![skill("visible")];
        let prompt = build_system_prompt(options);
        assert!(prompt.contains("<name>visible</name>"));
        assert!(prompt.contains(&format!("Use {tool} to load a skill's file")));
    }
}

#[test]
fn context_path_escapes_xml_attribute_characters() {
    let mut options = options();
    options.context_files = vec![ContextFile {
        path: r#"docs/quotes"<>&/AGENTS.md"#.into(),
        content: "rules".into(),
    }];

    let prompt = build_system_prompt(options);

    assert!(
        prompt
            .contains(r#"<project_instructions path="docs/quotes&quot;&lt;&gt;&amp;/AGENTS.md">"#)
    );
}

#[test]
fn append_prompt_is_separated_and_cwd_normalizes_slashes() {
    let mut options = options();
    options.cwd = "C:\\work\\project".into();
    options.append_system_prompt = Some("append".into());

    let prompt = build_system_prompt(options);
    assert!(prompt.contains("\n\nappend\nCurrent working directory: C:/work/project"));
    assert!(prompt.ends_with("Current working directory: C:/work/project"));
}

#[test]
fn cwd_escapes_markup_and_control_characters() {
    let mut options = options();
    options.cwd = "work\\path\n<inject>&\"".into();

    let prompt = build_system_prompt(options);

    assert!(prompt.contains("Current working directory: work/path\\n&lt;inject&gt;&amp;&quot;"));
    assert!(!prompt.contains("Current working directory: work/path\n<inject>"));
}
