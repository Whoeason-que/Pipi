use super::escape_path_for_prompt;

#[test]
fn path_escaping_preserves_prompt_structure() {
    assert_eq!(
        escape_path_for_prompt("C:\\work\n<unsafe>&\"'"),
        "C:/work\\n&lt;unsafe&gt;&amp;&quot;&apos;"
    );
}
