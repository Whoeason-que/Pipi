//! 权限策略的端到端（黑盒）回归：只经 `PermissionsConfig` 的公开评估入口，
//! 断言白/黑名单、沙箱与强制删除的边界；白盒解析测试留在各模块内部。
//!
//! 改判据（重定向只拦写入/读入、`rm -f` 家族只放行工作区字面量）时同步改这里。

use std::fs;
use std::path::Path;

use pipi_tools::permissions::{
    AGENT_TOOLS, BACKGROUND_TASK_TOOLS, BashAssessment, BashMode, BashPermissions, DEFAULT_TOOLS,
    KNOWN_TOOLS, PermissionsConfig, SandboxMode,
};

/// 与 `config::default_tools` 等价：默认工具名列表（crate 内是 pub(super)）。
fn default_tools() -> Vec<String> {
    DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect()
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
