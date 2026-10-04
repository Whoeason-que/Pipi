use super::*;

#[test]
fn detects_image_magic_bytes() {
    assert_eq!(
        detect_image_mime(&[0x89, b'P', b'N', b'G', 0x0D]),
        Some("image/png")
    );
    assert_eq!(
        detect_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
        Some("image/jpeg")
    );
    assert_eq!(detect_image_mime(b"GIF89a...."), Some("image/gif"));
    // RIFF....WEBP：infer 需要完整的容器头
    assert_eq!(
        detect_image_mime(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
        Some("image/webp")
    );
    assert_eq!(
        detect_image_mime(b"BM\x36\x00\x00\x00\x00\x00"),
        Some("image/bmp")
    );
    // 不在附件白名单内的格式（如 SVG 文本、TIFF）不当作图片
    assert_eq!(
        detect_image_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
        None
    );
    assert_eq!(detect_image_mime(b"II*\x00\x08\x00\x00\x00"), None);
    assert_eq!(detect_image_mime(b"hello"), None);
}

async fn read_fixture() -> (
    std::path::PathBuf,
    ToolContext,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    use crate::permissions::PermissionsConfig;
    use crate::types::AbortSignal;
    use std::sync::Arc;

    let base = std::env::temp_dir().join(format!(
        "pipi-read-containment-{}",
        crate::session::new_id()
    ));
    let workspace = base.join("workspace");
    let skill_root = base.join("skills");
    let outside = base.join("outside.txt");
    tokio::fs::create_dir_all(&skill_root).await.unwrap();
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    tokio::fs::write(workspace.join("inside.txt"), "inside")
        .await
        .unwrap();
    let skill_file = skill_root.join("trusted.md");
    tokio::fs::write(&skill_file, "trusted skill")
        .await
        .unwrap();
    tokio::fs::write(&outside, "outside").await.unwrap();

    let ctx = ToolContext {
        workspace,
        memory_dir: None,
        read_roots: vec![skill_root],
        permissions: Arc::new(PermissionsConfig::default()),
        sandbox: crate::permissions::SandboxMode::DangerFullAccess,
        resolved_env: Arc::new(std::collections::BTreeMap::new()),
        abort: AbortSignal::new(),
        approver: None,
    };
    (base, ctx, skill_file, outside)
}

async fn read(ctx: &ToolContext, path: String) -> Result<ToolOutput, String> {
    ReadTool
        .execute(ctx, &json!({ "path": path }), &|_| {})
        .await
}

#[tokio::test]
async fn read_is_contained_by_workspace_and_resolves_canonical_target() {
    let (base, ctx, skill_file, outside) = read_fixture().await;

    let inside = read(&ctx, "inside.txt".into()).await.unwrap();
    assert!(matches!(
        &inside.content[0],
        ToolResultContent::Text { text } if text == "inside"
    ));
    let skill = read(&ctx, skill_file.to_string_lossy().into_owned())
        .await
        .unwrap();
    assert!(matches!(
        &skill.content[0],
        ToolResultContent::Text { text } if text == "trusted skill"
    ));
    assert!(
        read(&ctx, outside.to_string_lossy().into_owned())
            .await
            .is_err()
    );
    assert!(read(&ctx, "../outside.txt".into()).await.is_err());

    tokio::fs::remove_dir_all(base).await.unwrap();
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn read_rejects_symlink_to_outside_workspace() {
    let (base, ctx, _skill_file, outside) = read_fixture().await;
    let link = ctx.workspace.join("outside-link.txt");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&outside, &link).unwrap();

    assert!(read(&ctx, "outside-link.txt".into()).await.is_err());

    tokio::fs::remove_dir_all(base).await.unwrap();
}
