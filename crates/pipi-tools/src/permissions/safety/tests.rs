use super::*;

fn vec_str(items: &[&str]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

#[test]
fn rm_rf_is_dangerous() {
    assert_eq!(
        dangerous_command_match(&vec_str(&["rm", "-rf", "/"])),
        Some(DangerousCommandMatch::ForcedRm)
    );
    assert_eq!(
        dangerous_command_match(&vec_str(&["rm", "-f", "/"])),
        Some(DangerousCommandMatch::ForcedRm)
    );
}

#[test]
fn forced_rm_variants_are_dangerous() {
    for command in [
        vec_str(&["/bin/rm", "-fr", "/tmp/example"]),
        vec_str(&["rm", "-r", "-f", "/tmp/example"]),
        vec_str(&["rm", "--force", "/tmp/example"]),
        vec_str(&["rm", "/tmp/example", "-f"]),
        vec_str(&["sudo", "rm", "-rf", "/tmp/example"]),
        vec_str(&["env", "TARGET=/tmp/example", "rm", "-rf", "/tmp/example"]),
        vec_str(&["trap", "rm -rf /tmp/example", "EXIT"]),
    ] {
        assert_eq!(
            dangerous_command_match(&command),
            Some(DangerousCommandMatch::ForcedRm),
            "{command:?}"
        );
    }
}

#[test]
fn deeply_nested_command_wrappers_fail_closed() {
    for (depth, expected) in [
        (
            MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH,
            DangerousCommandMatch::ForcedRm,
        ),
        (
            MAX_DANGEROUS_COMMAND_WRAPPER_DEPTH + 1,
            DangerousCommandMatch::Other,
        ),
    ] {
        let command = std::iter::repeat_n("env", depth)
            .chain(["rm", "-rf", "/tmp/example"])
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(dangerous_command_match(&command), Some(expected));
    }
}

#[test]
fn forced_rm_in_shell_scripts_is_dangerous() {
    for script in [
        "echo x && rm -rf /tmp/example",
        "echo x; rm --force /tmp/example",
        "printf x | rm -rf /tmp/example",
        "bash -c 'rm -rf /tmp/example'",
    ] {
        let command = vec_str(&["bash", "-lc", script]);
        assert_eq!(
            dangerous_command_match(&command),
            Some(DangerousCommandMatch::ForcedRm),
            "{script}"
        );
    }
}

#[test]
fn unparseable_script_fails_closed() {
    // 引号未闭合 / 控制流粘连接符 —— 无法证明安全，按危险处理
    for script in [
        "if test -d /tmp/x; then rm -rf /tmp/x; fi",
        "echo \"$(rm x)",
    ] {
        let command = vec_str(&["bash", "-lc", script]);
        assert_eq!(
            dangerous_command_match(&command),
            Some(DangerousCommandMatch::Other),
            "{script}"
        );
    }
}

#[test]
fn non_forced_or_non_literal_rm_is_not_dangerous() {
    for command in [
        vec_str(&["rm", "-r", "/tmp/example"]),
        vec_str(&["rm", "--", "-f"]),
        vec_str(&["rm", "/tmp/file"]),
        vec_str(&["env", "TARGET=/tmp/x", "rm", "-r", "/tmp/example"]),
        vec_str(&["bash", "-lc", "rm -r /tmp/example"]),
    ] {
        assert_eq!(dangerous_command_match(&command), None, "{command:?}");
    }
}
