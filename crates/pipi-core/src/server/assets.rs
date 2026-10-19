use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

use super::AppState;

pub struct WebAsset {
    pub bytes: Vec<u8>,
    pub content_type: String,
}
pub type AssetResolver = Arc<dyn Fn(&str) -> Option<WebAsset> + Send + Sync>;

#[derive(Clone)]
pub enum WebAssets {
    Directory(PathBuf),
    Embedded(AssetResolver),
}

impl WebAssets {
    pub fn has_index(&self) -> bool {
        match self {
            Self::Directory(path) => path.join("index.html").is_file(),
            Self::Embedded(resolve) => resolve("index.html").is_some(),
        }
    }
}

pub(super) async fn embedded_asset(State(state): State<AppState>, uri: Uri) -> Response {
    let WebAssets::Embedded(resolve) = &state.assets else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = uri.path().trim_start_matches('/');
    if path.split('/').any(|part| part == ".." || part == ".") || path.contains(['\\', '%']) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let asset = resolve(if path.is_empty() { "index.html" } else { path }).or_else(|| {
        (!path.rsplit('/').next().unwrap_or("").contains('.'))
            .then(|| resolve("index.html"))
            .flatten()
    });
    match asset {
        Some(asset) => (
            [
                (header::CONTENT_TYPE, asset.content_type),
                (header::CACHE_CONTROL, "no-cache".into()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            ],
            Body::from(asset.bytes),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
