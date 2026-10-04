use super::*;

#[test]
fn percent_encoding() {
    assert_eq!(percent_encode("pdf"), "pdf");
    assert_eq!(percent_encode("a b"), "a%20b");
    assert_eq!(percent_encode("中文"), "%E4%B8%AD%E6%96%87");
    assert_eq!(percent_encode("a/b?c"), "a%2Fb%3Fc");
}

#[test]
fn empty_query_rejected() {
    assert!(search_skills("  ", 10).is_err());
}

#[test]
fn response_parsing() {
    let body = r#"{"skills":[{"id":"o/r/pdf","name":"pdf","installs":123,"source":"o/r"}]}"#;
    let parsed: SearchResponse = serde_json::from_str(body).unwrap();
    let item = &parsed.skills[0];
    assert_eq!(item.name, "pdf");
    assert_eq!(item.source, "o/r");
    assert_eq!(item.id.as_deref(), Some("o/r/pdf"));

    // 缺字段时用默认值（installs 缺失不报错）
    let body = r#"{"skills":[{"name":"x","source":"a/b"}]}"#;
    let parsed: SearchResponse = serde_json::from_str(body).unwrap();
    assert_eq!(parsed.skills[0].installs, 0);
}
