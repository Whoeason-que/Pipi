use std::collections::{HashMap, HashSet};

/// Pi-compatible system prompt inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildSystemPromptOptions {
    pub custom_prompt: Option<String>,
    pub selected_tools: Option<Vec<String>>,
    pub tool_snippets: HashMap<String, String>,
    pub prompt_guidelines: Vec<String>,
    pub append_system_prompt: Option<String>,
    pub cwd: String,
    pub context_files: Vec<ContextFile>,
    pub skills: Vec<SkillMetadata>,
}

/// 预加载的项目上下文文件；正文由宿主负责读取，builder 只负责渲染。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

/// 常驻 system prompt 的技能元信息；SKILL.md 正文仍按需读取。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: Option<String>,
    /// `true` 时只允许显式调用，不注入模型可见技能索引。
    pub disable_model_invocation: bool,
    pub path: String,
}

/// 根据 Pi coding-agent 的规则构造 system prompt。
///
/// 该函数是纯渲染层：不读取文件、不访问 session，也不执行权限检查。
pub fn build_system_prompt(options: BuildSystemPromptOptions) -> String {
    let BuildSystemPromptOptions {
        custom_prompt,
        selected_tools,
        tool_snippets,
        prompt_guidelines,
        append_system_prompt,
        cwd,
        context_files,
        skills,
    } = options;
    let prompt_cwd = crate::context::escape_path_for_prompt(&cwd);
    let tools = selected_tools.unwrap_or_else(|| {
        ["read", "bash", "edit", "write"]
            .into_iter()
            .map(str::to_string)
            .collect()
    });
    let skill_file_read_tool = if tools.iter().any(|tool| tool == "read") {
        Some("read")
    } else if tools.iter().any(|tool| tool == "bash") {
        Some("bash")
    } else {
        None
    };

    let append_section = append_system_prompt
        .filter(|text| !text.is_empty())
        .map(|text| format!("\n\n{text}"))
        .unwrap_or_default();

    let custom_prompt = custom_prompt.filter(|text| !text.is_empty());
    let has_custom_prompt = custom_prompt.is_some();
    let mut prompt = if let Some(custom_prompt) = custom_prompt {
        custom_prompt
    } else {
        default_prompt(&tools, &tool_snippets, &prompt_guidelines)
    };

    prompt.push_str(&append_section);
    append_context_files(&mut prompt, &context_files);
    if let Some(read_tool) = skill_file_read_tool {
        append_skills(&mut prompt, &skills, read_tool);
    }

    prompt.push_str("\nCurrent working directory: ");
    prompt.push_str(&prompt_cwd);
    if has_custom_prompt {
        prompt.push('\n');
    }
    prompt
}

fn default_prompt(
    tools: &[String],
    tool_snippets: &HashMap<String, String>,
    prompt_guidelines: &[String],
) -> String {
    let visible_tools = tools
        .iter()
        .filter_map(|name| {
            tool_snippets
                .get(name)
                .filter(|snippet| !snippet.is_empty())
                .map(|snippet| (name, snippet))
        })
        .map(|(name, snippet)| format!("- {name}: {snippet}"))
        .collect::<Vec<_>>();
    let tools_list = if visible_tools.is_empty() {
        "(none)".to_string()
    } else {
        visible_tools.join("\n")
    };

    let mut guidelines = Vec::new();
    let mut seen = HashSet::new();
    let mut add_guideline = |guideline: String| {
        if seen.insert(guideline.clone()) {
            guidelines.push(guideline);
        }
    };

    let has = |name: &str| tools.iter().any(|tool| tool == name);
    if (has("bash") || has("powershell")) && !has("grep") && !has("find") && !has("ls") {
        if has("bash") && has("powershell") {
            add_guideline(
                "Use bash or PowerShell for file operations like listing, searching, and finding files"
                    .to_string(),
            );
        } else if has("powershell") {
            add_guideline(
                "Use PowerShell for file operations like listing, searching, and finding files"
                    .to_string(),
            );
        } else {
            add_guideline("Use bash for file operations like ls, rg, find".to_string());
        }
    }
    for guideline in prompt_guidelines {
        let normalized = guideline.trim();
        if !normalized.is_empty() {
            add_guideline(normalized.to_string());
        }
    }
    add_guideline("Be concise in your responses".to_string());
    add_guideline("Show file paths clearly when working with files".to_string());

    format!(
        "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\nAvailable tools:\n{tools_list}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n\nGuidelines:\n{}",
        guidelines
            .iter()
            .map(|guideline| format!("- {guideline}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn append_context_files(prompt: &mut String, context_files: &[ContextFile]) {
    if context_files.is_empty() {
        return;
    }
    prompt.push_str("\n\n<project_context>\n\n");
    prompt.push_str("Project-specific instructions and guidelines:\n\n");
    for file in context_files {
        prompt.push_str(&format!(
            "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
            crate::context::escape_path_for_prompt(&file.path),
            file.content
        ));
    }
    prompt.push_str("</project_context>\n");
}

fn append_skills(prompt: &mut String, skills: &[SkillMetadata], read_tool: &str) {
    let visible_skills = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect::<Vec<_>>();
    if visible_skills.is_empty() {
        return;
    }
    prompt.push_str(
        "\n\nThe following skills provide specialized instructions for specific tasks.\n",
    );
    prompt.push_str(&format!(
        "Use {read_tool} to load a skill's file when the task matches its description.\n"
    ));
    prompt.push_str("When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\n<available_skills>\n");
    for skill in visible_skills {
        prompt.push_str("  <skill>\n");
        prompt.push_str(&format!("    <name>{}</name>\n", escape_xml(&skill.name)));
        prompt.push_str(&format!(
            "    <description>{}</description>\n",
            escape_xml(skill.description.as_deref().unwrap_or_default())
        ));
        prompt.push_str(&format!(
            "    <location>{}</location>\n",
            crate::context::escape_path_for_prompt(&skill.path)
        ));
        prompt.push_str("  </skill>\n");
    }
    prompt.push_str("</available_skills>");
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{build_system_prompt, BuildSystemPromptOptions, ContextFile, SkillMetadata};
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
        assert!(
            prompt.contains("<project_context>\n\nProject-specific instructions and guidelines:")
        );
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
        assert!(prompt
            .contains("<project_context>\n\nProject-specific instructions and guidelines:\n\n"));
        assert!(
            prompt.ends_with("</project_context>\n\nCurrent working directory: /work/project\n")
        );
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

        assert!(prompt
            .contains(r#"<project_instructions path="docs/quotes&quot;&lt;&gt;&amp;/AGENTS.md">"#));
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
}
