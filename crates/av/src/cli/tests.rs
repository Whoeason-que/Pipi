use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn arg_parsing_is_strict() {
    let parsed = parse_args(
        &args(&[
            "spec",
            "--skill",
            "pdf",
            "--skill",
            "writer",
            "--all",
            "--dry-run",
        ]),
        &["skill"],
        &["all", "dry-run"],
    )
    .unwrap();
    assert_eq!(parsed.positional, ["spec".to_string()]);
    assert_eq!(parsed.values_of("skill"), ["pdf", "writer"]);
    assert!(parsed.flag("all") && parsed.flag("dry-run"));

    // 未声明参数 / 缺少取值
    assert!(parse_args(&args(&["--nope"]), &[], &[]).is_err());
    assert!(parse_args(&args(&["--limit"]), &["limit"], &[]).is_err());
    assert_eq!(
        parse_args(&args(&["--limit", "7"]), &["limit"], &[])
            .unwrap()
            .usize_value("limit")
            .unwrap(),
        Some(7)
    );
    assert!(
        parse_args(&args(&["--limit", "x"]), &["limit"], &[])
            .unwrap()
            .usize_value("limit")
            .is_err()
    );
}
