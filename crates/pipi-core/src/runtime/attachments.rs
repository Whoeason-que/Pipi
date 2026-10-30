//! Validation shared by immediate, queued and steering messages.
use base64::{Engine, engine::general_purpose::STANDARD};
use pipi_protocol::{UserContent, UserContentBlock, UserImage};

const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const MAX_IMAGES: usize = 4;

pub(super) fn user_content(
    text: &str,
    images: &[UserImage],
) -> Result<Option<UserContent>, String> {
    if images.len() > MAX_IMAGES {
        return Err("At most 4 images can be attached".into());
    }
    let mut total = 0usize;
    let mut parts = vec![UserContentBlock::Text {
        text: text.trim().to_string(),
    }];
    for image in images {
        if image.data.len() > (MAX_IMAGE_BYTES + 2) / 3 * 4 {
            return Err("Attached images exceed the 1 MiB limit".into());
        }
        let bytes = STANDARD
            .decode(&image.data)
            .map_err(|_| "Invalid image encoding")?;
        total = total.saturating_add(bytes.len());
        if total > MAX_IMAGE_BYTES {
            return Err("Attached images exceed the 1 MiB limit".into());
        }
        let valid = match image.mime_type.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            _ => false,
        };
        if !valid {
            return Err("Invalid or unsupported image".into());
        }
        parts.push(UserContentBlock::Image {
            data: image.data.clone(),
            mime_type: image.mime_type.clone(),
        });
    }
    if text.trim().is_empty() && images.is_empty() {
        return Err("Message cannot be empty".into());
    }
    Ok((!images.is_empty()).then_some(UserContent::Parts(parts)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_image_content_and_bounds() {
        let image = UserImage {
            data: STANDARD.encode(b"\x89PNG\r\n\x1a\nimage"),
            mime_type: "image/png".into(),
        };
        assert!(
            user_content("", std::slice::from_ref(&image))
                .unwrap()
                .is_some()
        );
        assert!(user_content("", &[]).is_err());
        assert!(
            user_content(
                "x",
                &[UserImage {
                    mime_type: "image/jpeg".into(),
                    ..image.clone()
                }]
            )
            .is_err()
        );
        assert!(user_content("x", &vec![image; 5]).is_err());
    }
}
