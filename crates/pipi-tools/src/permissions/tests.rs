use super::*;
use std::fs;
use std::path::Path;

#[test]
fn split_respects_quotes() {
    let segs = split_segments(r#"git commit -m "a; rm -rf /" && ls"#).unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].argv, vec!["git", "commit", "-m", "a; rm -rf /"]);
    assert_eq!(segs[1].argv, vec!["ls"]);
}

#[test]
fn glued_connectors_split_like_bash() {
    // bash 本来就把 a&&rm 解析为两个命令 —— 同样切分，交给后续层拦截
    let segs = split_segments("echo a&&rm -rf /").unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[1].argv, vec!["rm", "-rf", "/"]);
    assert!(split_segments("echo \"unclosed").is_err());
    assert!(split_segments("   ").is_err());
}

#[test]
fn quoted_metacharacters_are_plain_text() {
    // 引号里的 & ; | 是普通字符：shlex 分词后仍在同一个参数里
    let segs = split_segments(r#"echo "a & b"; echo c'|'d"#).unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].argv, vec!["echo", "a & b"]);
    assert_eq!(segs[1].argv, vec!["echo", "c|d"]);
}

#[test]
fn background_ampersand_splits() {
    let segs = split_segments("sleep 5 & echo done").unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].argv, vec!["sleep", "5"]);
    assert_eq!(segs[1].argv, vec!["echo", "done"]);
}

#[test]
fn allowlist_matches_word_prefix() {
    let p = PermissionsConfig {
        tools: default_tools(),
        bash: BashPermissions {
            mode: BashMode::Allowlist,
            commands: vec!["git".into(), "npm run".into()],
        },
        sandbox: SandboxMode::DangerFullAccess,
    };
    let allowed = |c: &str| {
        matches!(
            p.assess_bash_classified(c, Path::new("/tmp")),
            BashAssessment::Allowed
        )
    };
    let askable = |c: &str| {
        matches!(
            p.assess_bash_classified(c, Path::new("/tmp")),
            BashAssessment::NeedsApproval { .. }
        )
    };
    assert!(allowed("git status"));
    assert!(allowed("git commit -m 'x'"));
    assert!(allowed("npm run build"));
    assert!(askable("  ls -la"));
}

#[test]
fn allowlist_checks_compound_segments() {
    let p = PermissionsConfig {
        tools: default_tools(),
        bash: BashPermissions {
            mode: BashMode::Allowlist,
            commands: vec!["git".into()],
        },
        sandbox: SandboxMode::DangerFullAccess,
    };
    assert!(
        p.assess_bash("git add . && git commit", Path::new("/tmp"))
            .is_ok()
    );
    assert!(
        p.assess_bash("git add . && rm -rf /tmp/x", Path::new("/tmp"))
            .is_err()
    );
    assert!(
        p.assess_bash("echo hi; git status", Path::new("/tmp"))
            .is_err()
    );
}

#[test]
fn denylist_blocks_matching_segment() {
    let p = PermissionsConfig {
        tools: default_tools(),
        bash: BashPermissions {
            mode: BashMode::Denylist,
            commands: vec!["rm".into(), "sudo".into()],
        },
        sandbox: SandboxMode::DangerFullAccess,
    };
    assert!(
        p.assess_bash("ls -la && rm -rf /tmp/x", Path::new("/tmp"))
            .is_err()
    );
    assert!(
        p.assess_bash("sudo apt install x", Path::new("/tmp"))
            .is_err()
    );
    assert!(
        p.assess_bash("ls -la && git status", Path::new("/tmp"))
            .is_ok()
    );
}

#[test]
fn quotes_defeat_naive_denylist_but_not_ours() {
    let p = PermissionsConfig {
        tools: default_tools(),
        bash: BashPermissions {
            mode: BashMode::Denylist,
            commands: vec!["rm".into()],
        },
        sandbox: SandboxMode::DangerFullAccess,
    };
    // 引号里的 "分号" 不拆段：整段是一条 echo，不命中 rm
    assert!(
        p.assess_bash(r#"echo "a; rm -rf /""#, Path::new("/tmp"))
            .is_ok()
    );
    assert!(p.assess_bash("rm x", Path::new("/tmp")).is_err());
}

#[test]
fn sandbox_blocks_forced_rm_unless_full_access() {
    // workspace-write：工作区外的强制删除被拒，普通 rm 放行
    let mut p = PermissionsConfig {
        sandbox: SandboxMode::WorkspaceWrite,
        ..Default::default()
    };
    assert!(
        p.assess_bash("rm -rf /var/tmp/pipi-outside", Path::new("/tmp"))
            .is_err()
    );
    assert!(p.assess_bash("rm -r build/", Path::new("/tmp")).is_ok());
    // 工作区内的字面量强制删除放行 —— 边界与反例见
    // forced_rm_is_allowed_only_for_workspace_literals
    assert!(p.assess_bash("rm -rf build/", Path::new("/tmp")).is_ok());
    // danger-full-access：放行
    p.sandbox = SandboxMode::DangerFullAccess;
    assert!(
        p.assess_bash("rm -rf /var/tmp/pipi-outside", Path::new("/tmp"))
            .is_ok()
    );
}

#[test]
fn read_only_blocks_bash_entirely() {
    let p = PermissionsConfig {
        tools: default_tools(),
        bash: BashPermissions::default(),
        sandbox: SandboxMode::ReadOnly,
    };
    assert!(p.assess_bash("ls", Path::new("/tmp")).is_err());
}

#[test]
fn workspace_write_rejects_shell_redirects_before_execution() {
    let root = std::env::temp_dir().join(format!(
        "pipi-redirect-policy-{}",
        pipi_test_support::new_id()
    ));
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let p = PermissionsConfig {
        tools: default_tools(),
        bash: BashPermissions::default(),
        sandbox: SandboxMode::WorkspaceWrite,
    };

    for command in [
        "ls > out.txt",
        "ls >| out.txt",
        "ls &> out.txt",
        "ls foo>out.txt",
        "cat < /etc/passwd",
        "ln -s /outside link && printf x > link",
    ] {
        assert!(
            p.assess_bash(command, &ws).is_err(),
            "redirect must be rejected: {command}"
        );
    }
    assert!(p.assess_bash("printf '>'", &ws).is_ok());
    assert!(p.assess_bash(r#"printf ">""#, &ws).is_ok());
    assert!(p.assess_bash(r"printf \>", &ws).is_ok());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn serde_roundtrip_and_legacy_default() {
    let cfg = PermissionsConfig {
        tools: vec!["read".into(), "bash".into()],
        bash: BashPermissions {
            mode: BashMode::Allowlist,
            commands: vec!["git".into()],
        },
        sandbox: SandboxMode::WorkspaceWrite,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(json.contains("workspace-write"));
    let back: PermissionsConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cfg);

    // 旧文件：没有 sandbox 字段 → DangerFullAccess（行为不变）
    let legacy = r#"{"tools":["read"],"bash":{"mode":"allowAll","commands":[]}}"#;
    let back: PermissionsConfig = serde_json::from_str(legacy).unwrap();
    assert_eq!(back.sandbox, SandboxMode::DangerFullAccess);
}

#[test]
fn path_containment() {
    let base = Path::new("/home/u/project");
    assert!(is_within(base, Path::new("/home/u/project/a/b.txt")));
    assert!(is_within(base, Path::new("/home/u/project/../project/x")));
    assert!(!is_within(base, Path::new("/home/u/other")));
    assert!(!is_within(base, Path::new("/home/u/project/../..")));
    assert!(!is_within(base, Path::new("/etc/passwd")));
}

#[test]
fn agent_composition_tools_are_known_but_never_defaulted() {
    let defaults = PermissionsConfig::default();
    for tool in AGENT_TOOLS {
        assert!(KNOWN_TOOLS.contains(&tool));
        assert!(!defaults.tool_enabled(tool));
    }
    for tool in BACKGROUND_TASK_TOOLS {
        assert!(KNOWN_TOOLS.contains(&tool));
        assert!(!defaults.tool_enabled(tool));
    }

    let legacy: PermissionsConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.tools, DEFAULT_TOOLS);
}

fn allowlist_config(commands: &[&str], sandbox: SandboxMode) -> PermissionsConfig {
    PermissionsConfig {
        tools: vec!["bash".into()],
        bash: BashPermissions {
            mode: BashMode::Allowlist,
            commands: commands.iter().map(|c| c.to_string()).collect(),
        },
        sandbox,
    }
}

#[test]
fn classification_allowlist_miss_is_askable() {
    let cfg = allowlist_config(&["git"], SandboxMode::WorkspaceWrite);
    assert_eq!(
        cfg.assess_bash_classified("git status", Path::new("/tmp")),
        BashAssessment::Allowed
    );
    assert_eq!(
        cfg.assess_bash_classified("npm install", Path::new("/tmp")),
        BashAssessment::NeedsApproval {
            missing: vec!["npm install".into()]
        }
    );
    // 复合命令：只收集未命中的段落
    assert_eq!(
        cfg.assess_bash_classified("git status && npm install", Path::new("/tmp")),
        BashAssessment::NeedsApproval {
            missing: vec!["npm install".into()]
        }
    );
}

#[test]
fn classification_dangerous_and_denied_are_never_askable() {
    let cfg = allowlist_config(&["git"], SandboxMode::WorkspaceWrite);
    // 白名单未命中 + 危险命令 → 硬拒
    match cfg.assess_bash_classified("rm -rf /tmp/x", Path::new("/tmp")) {
        BashAssessment::HardDenied(message) => assert!(message.contains("拒绝")),
        other => panic!("expected HardDenied, got {other:?}"),
    }
    // denylist 命中 → 硬拒
    let deny = PermissionsConfig {
        tools: vec!["bash".into()],
        bash: BashPermissions {
            mode: BashMode::Denylist,
            commands: vec!["curl".into()],
        },
        sandbox: SandboxMode::DangerFullAccess,
    };
    assert!(matches!(
        deny.assess_bash_classified("curl example.com", Path::new("/tmp")),
        BashAssessment::HardDenied(_)
    ));
    // 沙箱重定向 → 硬拒
    assert!(matches!(
        cfg.assess_bash_classified("git status > out.txt", Path::new("/tmp")),
        BashAssessment::HardDenied(_)
    ));
    // read-only → 一律硬拒
    let ro = allowlist_config(&["git"], SandboxMode::ReadOnly);
    assert!(matches!(
        ro.assess_bash_classified("git status", Path::new("/tmp")),
        BashAssessment::HardDenied(_)
    ));
}

#[test]
fn classification_assess_bash_maps_miss_to_error() {
    let cfg = allowlist_config(&["git"], SandboxMode::WorkspaceWrite);
    let error = cfg
        .assess_bash("npm install", Path::new("/tmp"))
        .unwrap_err();
    assert!(error.contains("不在白名单中"));
}

// ---------- 沙箱重定向：只有写入文件系统的重定向才算命中 ----------

#[test]
fn write_redirect_allows_non_writing_forms() {
    for command in [
        // 丢弃 / 合并输出
        "duckdb -c 'select 1' 2>/dev/null",
        "timeout 900 cargo test -p x --lib 2>&1 | tail -40",
        "make >>/dev/null",
        "make > /dev/null",
        "cmd 2>&1",
        "cmd 1>&2",
        "cmd >&2",
        "cmd 2>&-",
        "cmd 2>>/dev/null",
        "cmd 2>> \"/dev/null\"",
        "cmd 2>&2 >>/dev/null",
        // heredoc / herestring：喂 stdin，正文按数据处理
        "cmd <<<word",
        "python3 - <<'PY'\nprint(1 > 0)\nPY",
        "python3 - <<'PY'\nrows = [x for x in y if x->z]\nPY",
        "python3 - <<-'PY'\n\tprint(\"a\")\n\tPY",
        "put <<A <<B\nbody A\nA\nbody B\nB",
        "cat <<EOF\n> out.txt && rm -f /tmp/whatever\nEOF",
        // 引号 / 注释 / 转义里的操作符
        "echo \"a > b\"",
        "grep '>' file.txt",
        "echo hi # > not-a-redirect",
        "printf \\>",
        "cmd | grep x",
    ] {
        assert!(
            matches!(check_write_redirect(command), RedirectCheck::Ok),
            "应放行：{command}"
        );
    }
}

#[test]
fn write_redirect_flags_filesystem_writes() {
    for (command, token) in [
        ("echo hi > out.txt", ">out.txt"),
        ("make >> build.log", ">>build.log"),
        ("cmd 2> err.log", "2>err.log"),
        ("cmd &> all.log", "&>all.log"),
        ("cmd >| force.txt", ">|force.txt"),
        ("cmd <> rw.txt", "<>rw.txt"),
        ("cmd >& file.txt", ">&file.txt"),
        ("exec > trace.log", ">trace.log"),
        ("cmd 2>&1 > out.txt", ">out.txt"),
        ("cmd > $OUT", ">$OUT"),
        ("cmd > \"quoted path.txt\"", ">quoted path.txt"),
        ("cmd >", ">"),
        ("echo 'a > b' > c", ">c"),
        ("ls foo>out.txt", ">out.txt"),
        ("duckdb -c 'select 1' > /tmp/x 2>/dev/null", ">/tmp/x"),
        ("ln -s /outside link && printf x > link", ">link"),
    ] {
        match check_write_redirect(command) {
            RedirectCheck::Write(found) => assert_eq!(found, token, "{command}"),
            RedirectCheck::Ok => panic!("应拒绝：{command}"),
            RedirectCheck::Read(found) => panic!("{command} 应为写入而非读入：{found}"),
            RedirectCheck::Unparseable(reason) => panic!("{command} 不应不可分析：{reason}"),
        }
    }
}

/// 输入侧重定向保持拒绝：与 read 工具的工作区包含检查同向，不放宽。
#[test]
fn write_redirect_flags_file_reads() {
    for (command, token) in [
        ("cat < /etc/passwd", "</etc/passwd"),
        ("wc -l < file.txt", "<file.txt"),
        ("cmd <&0", "<&0"),
        ("diff <(ls a) <(ls b)", "<(ls a)"),
        ("cmd < /dev/zero", "</dev/zero"),
    ] {
        match check_write_redirect(command) {
            RedirectCheck::Read(found) => assert_eq!(found, token, "{command}"),
            RedirectCheck::Ok => panic!("应拒绝：{command}"),
            RedirectCheck::Write(found) => panic!("{command} 应为读入而非写入：{found}"),
            RedirectCheck::Unparseable(reason) => panic!("{command} 不应不可分析：{reason}"),
        }
    }
    // 读 /dev/null 等于读不到东西，按丢弃处理
    assert!(matches!(
        check_write_redirect("cmd < /dev/null"),
        RedirectCheck::Ok
    ));
}

#[test]
fn write_redirect_fails_closed_on_unanalyzable_forms() {
    for command in [
        "cmd <<EOF",         // heredoc 没有正文 / 结束符
        "cmd <<EOF\nbody\n", // 正文跑完也没见到结束符
        "cmd <(a",           // 进程替换括号不配对
    ] {
        assert!(
            matches!(check_write_redirect(command), RedirectCheck::Unparseable(_)),
            "应判为不可分析：{command}"
        );
    }
    // 未闭合引号不在这里判死：段落切分会以同样的理由拒绝整条命令
    assert!(matches!(
        check_write_redirect("echo \"unclosed > out"),
        RedirectCheck::Ok
    ));
    assert!(split_segments("echo \"unclosed > out").is_err());
}

#[test]
fn assessment_reports_the_offending_redirect() {
    let cfg = workspace_write_config();
    assert_eq!(
        cfg.assess_bash_classified("timeout 900 cargo test 2>&1 | tail -40", Path::new("/tmp")),
        BashAssessment::Allowed
    );
    let denied = cfg.assess_bash_classified("cargo test > out.txt", Path::new("/tmp"));
    let BashAssessment::HardDenied(message) = denied else {
        panic!("写入重定向必须硬拒");
    };
    assert!(message.contains("「>out.txt」"), "{message}");
    // 报错要教模型怎么改：点明允许的构造
    assert!(message.contains("2>/dev/null"), "{message}");
}

fn workspace_write_config() -> PermissionsConfig {
    PermissionsConfig {
        tools: vec!["bash".into()],
        bash: BashPermissions {
            mode: BashMode::Denylist,
            commands: Vec::new(),
        },
        sandbox: SandboxMode::WorkspaceWrite,
    }
}

// ---------- 强制删除：工作区内的字面量目标 ----------

#[test]
fn forced_rm_is_allowed_only_for_workspace_literals() {
    let root = std::env::temp_dir().join(format!("pipi-perm-{}", pipi_test_support::new_id()));
    let workspace = root.join("ws");
    fs::create_dir_all(workspace.join(".cache/bench3/tmp")).unwrap();
    fs::create_dir_all(workspace.join("Polymarket/.git")).unwrap();
    fs::write(workspace.join("stale.txt"), "x").unwrap();
    let cfg = workspace_write_config();
    let cache_tmp = workspace.join(".cache/bench3/tmp");

    let allowed = [
        format!("rm -rf {}", cache_tmp.display()),
        format!("rm -f {}", workspace.join("stale.txt").display()),
        format!("rm -rf -- {}", cache_tmp.display()),
        format!("rm -fr {}", cache_tmp.display()),
        "rm -rf .cache/bench3/tmp".to_string(),
    ];
    for command in allowed {
        assert_eq!(
            cfg.assess_bash_classified(&command, &workspace),
            BashAssessment::Allowed,
            "应放行：{command}"
        );
    }

    let denied = [
        "rm -rf /".to_string(),
        "rm -rf /tmp".to_string(),
        format!("rm -rf {}/..", workspace.display()),
        format!("rm -rf {}", workspace.display()), // 工作区根自身
        format!("rm -rf {}", workspace.join("Polymarket").display()), // git 仓库根
        format!("rm -rf {}/.cache/bench3/*", workspace.display()),
        "rm -rf $HOME/x".to_string(),
        "rm -rf ~/x".to_string(),
        "rm -rf $DIR/bench3/tmp".to_string(), // 变量
        "cd /tmp && rm -rf .cache/bench3/tmp".to_string(), // 含 cd 的相对路径
        format!("sudo rm -rf {}", cache_tmp.display()),
        format!("bash -c 'rm -rf {}'", cache_tmp.display()),
        format!("rm -rf {} --one-file-system", cache_tmp.display()),
    ];
    for command in denied {
        assert!(
            matches!(
                cfg.assess_bash_classified(&command, &workspace),
                BashAssessment::HardDenied(_)
            ),
            "应拒绝：{command}"
        );
    }

    fs::remove_dir_all(&root).ok();
}
