use super::*;

fn use_of(text: &str) -> Option<Vec<String>> {
    let config: AgentToml = toml::from_str(text).unwrap();
    config
        .resources
        .and_then(|resources| resources.skills)
        .and_then(|skills| skills.use_)
}

#[test]
fn add_and_remove_use_names_preserve_format() {
    let text = "# 契约\nschema = 1\n\n[env]\nset = { A = \"1\" }\n";
    let (edited, changed) = edit_use_names(text, &["pdf".into(), "writer".into()], &[]).unwrap();
    assert!(changed);
    assert!(edited.contains("# 契约"), "注释保留：{edited}");
    assert!(
        edited.contains("set = { A = \"1\" }"),
        "既有内容保留：{edited}"
    );
    assert_eq!(use_of(&edited), Some(vec!["pdf".into(), "writer".into()]));

    // 幂等：重复添加不产生重复项，且不算变化
    let (edited, changed) = edit_use_names(&edited, &["pdf".into()], &[]).unwrap();
    assert!(!changed);
    assert_eq!(use_of(&edited), Some(vec!["pdf".into(), "writer".into()]));

    // 移除一个
    let (edited, changed) = edit_use_names(&edited, &[], &["pdf".into()]).unwrap();
    assert!(changed);
    assert_eq!(use_of(&edited), Some(vec!["writer".into()]));

    // 移除最后一个 → use 键消失，且空表被清理
    let (edited, changed) = edit_use_names(&edited, &[], &["writer".into()]).unwrap();
    assert!(changed);
    assert!(!edited.contains("use"), "{edited}");
    assert!(!edited.contains("[resources"), "空表应被清理：{edited}");
    assert!(use_of(&edited).is_none());

    // 移除不存在的名字 → 原样返回、不报变化
    let (edited, changed) = edit_use_names(&edited, &[], &["nope".into()]).unwrap();
    assert!(!changed);
    assert!(edited.contains("schema = 1"));
}

#[test]
fn existing_use_array_is_extended_in_place() {
    let text = "schema = 1\n[resources.skills]\nuse = [\"a\"]  # 手动维护\n";
    let (edited, changed) = edit_use_names(text, &["b".into()], &[]).unwrap();
    assert!(changed);
    assert!(edited.contains("# 手动维护"), "{edited}");
    assert_eq!(use_of(&edited), Some(vec!["a".into(), "b".into()]));
}

#[test]
fn inline_table_is_rejected_with_hint() {
    let text = "schema = 1\nresources = { max-bytes = 100 }\n";
    let error = edit_use_names(text, &["pdf".into()], &[]).unwrap_err();
    assert!(error.contains("标准表"), "{error}");
}

#[test]
fn edited_result_must_pass_schema_validation() {
    // 非法技能名（大写）在落盘前被 schema 拦下
    let text = "schema = 1\n";
    let error = edit_use_names(text, &["PDF".into()], &[]).unwrap_err();
    assert!(error.contains("形状非法"), "{error}");

    // 缺 schema 的文本无法通过校验（调用方负责先补齐 schema = 1）
    let error = edit_use_names("", &["pdf".into()], &[]).unwrap_err();
    assert!(error.contains("解析"), "{error}");
}
