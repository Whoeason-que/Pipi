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
    /// memory 渐进召回索引：路径 + 一句话摘要常驻，正文按需 read。
    pub memory_files: Vec<MemoryFileMeta>,
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

/// 常驻 system prompt 的 memory 索引条目；正文按需 read（渐进披露）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryFileMeta {
    /// memory 文件的绝对路径（模型直接用 read 加载）。
    pub path: String,
    /// 首个标题或首行非空文本（截断）。
    pub summary: String,
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
        memory_files,
    } = options;
    let prompt_cwd = crate::escape_path_for_prompt(&cwd);
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
        append_memory_index(&mut prompt, &memory_files, read_tool);
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
            crate::escape_path_for_prompt(&file.path),
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
            crate::escape_path_for_prompt(&skill.path)
        ));
        prompt.push_str("  </skill>\n");
    }
    prompt.push_str("</available_skills>");
}

fn append_memory_index(prompt: &mut String, memory_files: &[MemoryFileMeta], read_tool: &str) {
    if memory_files.is_empty() {
        return;
    }
    prompt.push_str(
        "\n\nYou also have persistent memory: Markdown files you and the user maintain together across sessions.\n",
    );
    prompt.push_str(&format!(
        "When a memory file may contain relevant preferences, background or lessons, use {read_tool} to load it before acting on assumptions.\n\n<persistent_memory>\n"
    ));
    for file in memory_files {
        prompt.push_str("  <memory>\n");
        prompt.push_str(&format!(
            "    <path>{}</path>\n",
            crate::escape_path_for_prompt(&file.path)
        ));
        if !file.summary.is_empty() {
            prompt.push_str(&format!(
                "    <summary>{}</summary>\n",
                escape_xml(&file.summary)
            ));
        }
        prompt.push_str("  </memory>\n");
    }
    prompt.push_str("</persistent_memory>");
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
mod tests;
