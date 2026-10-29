//! Read VS Code file icon themes from installed extensions. No extension code is executed.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconThemeInfo {
    pub id: String,
    pub label: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IconThemeList {
    pub selected: Option<String>,
    pub themes: Vec<IconThemeInfo>,
}
#[derive(Deserialize, Serialize)]
struct Selection {
    id: Option<String>,
}

fn selection_path() -> Result<PathBuf, String> {
    pipi_venv::av::paths::file_icon_theme_path()
        .ok_or_else(|| "Cannot locate Pipi data directory".into())
}
const SETI_THEME_ID: &str = "vscode.vscode-theme-seti:vs-seti";

fn saved_selection(path: &Path) -> Option<Selection> {
    read_limited(path, 16_000)
        .ok()
        .and_then(|b| serde_json::from_slice::<Selection>(&b).ok())
}

fn selected_in(path: Option<&Path>, themes: &[Candidate]) -> Option<String> {
    match path.and_then(saved_selection) {
        // A saved null is an explicit choice of Pipi's built-in icons.
        Some(selection) => selection.id,
        None => themes
            .iter()
            .find(|theme| theme.info.id == SETI_THEME_ID)
            .map(|theme| theme.info.id.clone()),
    }
}
fn extension_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".vscode/extensions"));
        roots.push(home.join(".vscode-insiders/extensions"));
    }
    #[cfg(target_os = "linux")]
    roots.push(PathBuf::from("/usr/share/code/resources/app/extensions"));
    #[cfg(target_os = "macos")]
    roots.push(PathBuf::from(
        "/Applications/Visual Studio Code.app/Contents/Resources/app/extensions",
    ));
    #[cfg(target_os = "windows")]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots
            .push(PathBuf::from(local).join("Programs/Microsoft VS Code/resources/app/extensions"));
    }
    roots
}
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file =
        std::fs::File::open(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > limit {
        return Err(format!("File exceeds size limit: {}", path.display()));
    }
    Ok(data)
}
fn inside(root: &Path, base: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.contains('\0') || Path::new(relative).is_absolute() {
        return Err("Invalid theme resource path".into());
    }
    let path = base
        .join(relative)
        .canonicalize()
        .map_err(|e| format!("Cannot resolve theme resource: {e}"))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err("Theme resource escapes its extension directory".into());
    }
    Ok(path)
}
// VS Code theme files are JSON with comments and optional trailing commas.
fn jsonc(bytes: &[u8]) -> Result<Value, String> {
    let input = std::str::from_utf8(bytes)
        .map_err(|e| e.to_string())?
        .trim_start_matches('\u{feff}');
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut quoted = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if quoted {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        if c == '"' {
            quoted = true;
            out.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for n in chars.by_ref() {
                if n == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = '\0';
            let mut closed = false;
            for n in chars.by_ref() {
                if prev == '*' && n == '/' {
                    closed = true;
                    break;
                }
                prev = n;
            }
            if !closed {
                return Err("Unterminated JSON comment".into());
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    let mut clean = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    quoted = false;
    escaped = false;
    while let Some(c) = chars.next() {
        if quoted {
            clean.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        if c == '"' {
            quoted = true;
            clean.push(c);
        } else if c == ',' {
            let mut look = chars.clone();
            while look.peek().is_some_and(|c| c.is_whitespace()) {
                look.next();
            }
            if !matches!(look.peek(), Some('}' | ']')) {
                clean.push(c);
            }
        } else {
            clean.push(c);
        }
    }
    serde_json::from_str(&clean).map_err(|e| format!("Invalid icon theme JSON: {e}"))
}
struct Candidate {
    info: IconThemeInfo,
    root: PathBuf,
    path: PathBuf,
    version: (Vec<u64>, bool),
}
fn version_key(version: &str) -> (Vec<u64>, bool) {
    let core = version.split(['-', '+']).next().unwrap_or("");
    (
        core.split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect(),
        !version.contains('-'),
    )
}
fn discover_in(roots: &[PathBuf]) -> Vec<Candidate> {
    let mut result = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut entries = entries.flatten().take(4096).collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
        for entry in entries {
            let Ok(dir) = entry.path().canonicalize() else {
                continue;
            };
            if !dir.is_dir() {
                continue;
            }
            let Ok(data) = read_limited(&dir.join("package.json"), 2_000_000) else {
                continue;
            };
            let Ok(manifest) = jsonc(&data) else { continue };
            let Some(items) = manifest
                .pointer("/contributes/iconThemes")
                .and_then(Value::as_array)
            else {
                continue;
            };
            let publisher = manifest["publisher"].as_str().unwrap_or("unknown");
            let name = manifest["name"].as_str().unwrap_or("extension");
            let version = version_key(manifest["version"].as_str().unwrap_or("0"));
            for item in items {
                let (Some(id), Some(rel)) = (item["id"].as_str(), item["path"].as_str()) else {
                    continue;
                };
                let Ok(path) = inside(&dir, &dir, rel) else {
                    continue;
                };
                let full_id = format!("{publisher}.{name}:{id}");
                let raw_label = item["label"].as_str().unwrap_or(id);
                let nls = read_limited(&dir.join("package.nls.json"), 500_000)
                    .ok()
                    .and_then(|bytes| jsonc(&bytes).ok());
                let label = raw_label
                    .strip_prefix('%')
                    .and_then(|s| s.strip_suffix('%'))
                    .and_then(|key| nls.as_ref()?.get(key)?.as_str())
                    .unwrap_or(raw_label);
                let candidate = Candidate {
                    info: IconThemeInfo {
                        id: full_id,
                        label: label.to_owned(),
                    },
                    root: dir.clone(),
                    path,
                    version: version.clone(),
                };
                if let Some(existing) = result
                    .iter_mut()
                    .find(|x: &&mut Candidate| x.info.id == candidate.info.id)
                {
                    if candidate.version > existing.version {
                        *existing = candidate;
                    }
                } else {
                    result.push(candidate);
                }
            }
        }
    }
    result.sort_by(|a, b| {
        a.info
            .label
            .to_lowercase()
            .cmp(&b.info.label.to_lowercase())
    });
    result
}
pub fn list() -> IconThemeList {
    let themes = discover_in(&extension_roots());
    IconThemeList {
        selected: selected_in(selection_path().ok().as_deref(), &themes),
        themes: themes.into_iter().map(|c| c.info).collect(),
    }
}
fn asset(root: &Path, base: &Path, name: &str) -> Result<String, String> {
    let path = inside(root, base, name)?;
    let mime = match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => return Err("Unsupported icon asset type".into()),
    };
    Ok(format!(
        "data:{mime};base64,{}",
        STANDARD.encode(read_limited(&path, 512_000)?)
    ))
}
fn validate_associations(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("Icon theme associations must be an object")?;
    for key in [
        "file",
        "folder",
        "folderExpanded",
        "rootFolder",
        "rootFolderExpanded",
    ] {
        if object.get(key).is_some_and(|item| !item.is_string()) {
            return Err(format!("Invalid icon association: {key}"));
        }
    }
    for key in [
        "fileNames",
        "fileExtensions",
        "folderNames",
        "folderNamesExpanded",
        "rootFolderNames",
        "rootFolderNamesExpanded",
        "languageIds",
    ] {
        if let Some(value) = object.get(key) {
            let map = value
                .as_object()
                .ok_or_else(|| format!("Invalid icon association map: {key}"))?;
            if map.values().any(|item| !item.is_string()) {
                return Err(format!("Invalid icon association map: {key}"));
            }
        }
    }
    Ok(())
}
fn validate_theme(data: &Value) -> Result<(), String> {
    validate_associations(data)?;
    for variant in ["light", "highContrast"] {
        if let Some(value) = data.get(variant) {
            validate_associations(value)?;
        }
    }
    let definitions = data
        .get("iconDefinitions")
        .and_then(Value::as_object)
        .ok_or("Icon theme must contain iconDefinitions")?;
    for def in definitions.values() {
        let object = def.as_object().ok_or("Invalid icon definition")?;
        for key in [
            "iconPath",
            "fontCharacter",
            "fontColor",
            "fontSize",
            "fontId",
        ] {
            if object.get(key).is_some_and(|value| !value.is_string()) {
                return Err(format!("Invalid icon definition field: {key}"));
            }
        }
    }
    if let Some(fonts) = data.get("fonts") {
        for font in fonts.as_array().ok_or("Invalid icon theme fonts")? {
            if font.get("id").and_then(Value::as_str).is_none() {
                return Err("Icon font must have an id".into());
            }
            if font.get("size").is_some_and(|value| !value.is_string()) {
                return Err("Invalid icon font size".into());
            }
            let sources = font
                .get("src")
                .and_then(Value::as_array)
                .ok_or("Invalid icon font sources")?;
            if sources
                .iter()
                .any(|source| source.get("path").and_then(Value::as_str).is_none())
            {
                return Err("Invalid icon font path".into());
            }
        }
    }
    Ok(())
}
fn load_candidate(candidate: &Candidate) -> Result<Value, String> {
    let base = candidate.path.parent().ok_or("Invalid theme path")?;
    let mut data = jsonc(&read_limited(&candidate.path, 2_000_000)?)?;
    validate_theme(&data)?;
    let mut total = 0usize;
    if let Some(definitions) = data
        .get_mut("iconDefinitions")
        .and_then(Value::as_object_mut)
    {
        for def in definitions.values_mut() {
            if let Some(path) = def.get("iconPath").and_then(Value::as_str) {
                // Missing/unsupported individual icons use the built-in fallback.
                let resolved = asset(&candidate.root, base, path).ok();
                if let Some(url) = resolved {
                    total += url.len();
                    def["iconPath"] = Value::String(url);
                } else {
                    def.as_object_mut().map(|d| d.remove("iconPath"));
                }
            }
            if total > 12_000_000 {
                return Err("Icon theme exceeds size limit".into());
            }
        }
    }
    if let Some(fonts) = data.get_mut("fonts").and_then(Value::as_array_mut) {
        for font in fonts {
            if let Some(sources) = font.get_mut("src").and_then(Value::as_array_mut) {
                for source in sources {
                    if let Some(path) = source.get("path").and_then(Value::as_str) {
                        if let Ok(url) = asset(&candidate.root, base, path) {
                            total += url.len();
                            source["path"] = Value::String(url);
                        } else {
                            source.as_object_mut().map(|s| s.remove("path"));
                        }
                    }
                }
            }
            if total > 12_000_000 {
                return Err("Icon theme exceeds size limit".into());
            }
        }
    }
    Ok(data)
}
fn load_in(id: Option<&str>, roots: &[PathBuf]) -> Result<Option<Value>, String> {
    let Some(id) = id else { return Ok(None) };
    let theme = discover_in(roots)
        .into_iter()
        .find(|item| item.info.id == id)
        .ok_or("Icon theme not found; install it in VS Code first")?;
    Ok(Some(load_candidate(&theme)?))
}
pub fn current() -> Result<Option<Value>, String> {
    let themes = discover_in(&extension_roots());
    let selected = selected_in(selection_path().ok().as_deref(), &themes);
    match selected {
        None => Ok(None),
        Some(id) => themes
            .iter()
            .find(|theme| theme.info.id == id)
            .ok_or_else(|| "Icon theme not found; install it in VS Code first".to_string())
            .and_then(load_candidate)
            .map(Some),
    }
}
fn select_at(id: Option<String>, roots: &[PathBuf], path: &Path) -> Result<Option<Value>, String> {
    let theme = load_in(id.as_deref(), roots)?;
    let json = serde_json::to_string_pretty(&Selection { id }).map_err(|e| e.to_string())?;
    pipi_venv::av::store::write_text_atomic(path, &json).map_err(|e| e.to_string())?;
    Ok(theme)
}
pub fn select(id: Option<String>) -> Result<Option<Value>, String> {
    select_at(id, &extension_roots(), &selection_path()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "linux")]
    fn loads_system_seti_when_installed() {
        let root = PathBuf::from("/usr/share/code/resources/app/extensions");
        if !root.join("theme-seti/package.json").is_file() {
            return;
        }
        let themes = discover_in(&[root]);
        let seti = themes
            .iter()
            .find(|theme| theme.info.id == "vscode.vscode-theme-seti:vs-seti")
            .expect("Seti should be discoverable");
        let loaded = load_candidate(seti).expect("Seti theme should load");
        assert!(
            loaded["fonts"][0]["src"][0]["path"]
                .as_str()
                .unwrap()
                .starts_with("data:font/woff;base64,")
        );
        assert_eq!(selected_in(None, &themes).as_deref(), Some(SETI_THEME_ID));
    }
    #[test]
    fn malformed_theme_is_rejected_before_reaching_the_frontend() {
        assert!(validate_theme(&serde_json::json!({"iconDefinitions": {}, "fonts": {}})).is_err());
        assert!(
            validate_theme(&serde_json::json!({"iconDefinitions": {"x": {"fontSize": []}}}))
                .is_err()
        );
        assert!(
            validate_theme(&serde_json::json!({"iconDefinitions": {}, "fileNames": {"x": 1}}))
                .is_err()
        );
    }
    #[test]
    fn newer_extension_versions_take_precedence() {
        assert!(version_key("1.10.0") > version_key("1.9.0"));
        assert!(version_key("1.10.0") > version_key("1.10.0-beta"));
    }
    #[test]
    fn jsonc_preserves_strings_and_accepts_comments_and_trailing_commas() {
        let value =
            jsonc(br#"{"url":"https://example.test/a//b", /* note */ "items":[1,2,],}"#).unwrap();
        assert_eq!(value["url"], "https://example.test/a//b");
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn discovers_and_loads_vscode_image_and_font_resources() {
        let dir = pipi_test_support::temp_dir("icon-theme-discovery");
        let extension = dir.join("sample");
        std::fs::create_dir_all(extension.join("icons")).unwrap();
        std::fs::write(extension.join("package.json"), r#"{"name":"icons","publisher":"author","contributes":{"iconThemes":[{"id":"demo","label":"%themeLabel%","path":"icons/theme.json"}]}}"#).unwrap();
        std::fs::write(
            extension.join("package.nls.json"),
            r#"{"themeLabel":"Demo theme"}"#,
        )
        .unwrap();
        std::fs::write(extension.join("icons/theme.json"), r#"{ // supported jsonc
          "iconDefinitions":{"a":{"iconPath":"./a.svg"},"b":{"fontCharacter":"\\E001"}},
          "fileExtensions":{"ts":"a"},"fonts":[{"id":"icons","src":[{"path":"./icons.woff","format":"woff"}]}],
        }"#).unwrap();
        std::fs::write(extension.join("icons/a.svg"), "<svg/>").unwrap();
        std::fs::write(extension.join("icons/icons.woff"), b"font-fixture").unwrap();
        let found = discover_in(std::slice::from_ref(&dir));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].info.id, "author.icons:demo");
        assert_eq!(found[0].info.label, "Demo theme");
        let selection = dir.join("settings/selection.json");
        assert!(
            select_at(
                Some("missing".into()),
                std::slice::from_ref(&dir),
                &selection
            )
            .is_err()
        );
        assert!(!selection.exists());
        let saved = select_at(
            Some("author.icons:demo".into()),
            std::slice::from_ref(&dir),
            &selection,
        )
        .unwrap();
        assert!(saved.is_some());
        let selection_data: Selection =
            serde_json::from_slice(&std::fs::read(&selection).unwrap()).unwrap();
        assert_eq!(selection_data.id.as_deref(), Some("author.icons:demo"));
        assert!(
            select_at(None, std::slice::from_ref(&dir), &selection)
                .unwrap()
                .is_none()
        );
        let selection_data: Selection =
            serde_json::from_slice(&std::fs::read(&selection).unwrap()).unwrap();
        assert!(selection_data.id.is_none());
        let theme = load_candidate(&found[0]).unwrap();
        assert!(
            theme["iconDefinitions"]["a"]["iconPath"]
                .as_str()
                .unwrap()
                .starts_with("data:image/svg+xml;base64,")
        );
        assert!(
            theme["fonts"][0]["src"][0]["path"]
                .as_str()
                .unwrap()
                .starts_with("data:font/woff;base64,")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn default_theme_respects_explicit_builtin_selection() {
        let dir = pipi_test_support::temp_dir("icon-theme-default");
        let selection = dir.join("selection.json");
        let themes = vec![Candidate {
            info: IconThemeInfo {
                id: SETI_THEME_ID.into(),
                label: "Seti".into(),
            },
            root: dir.clone(),
            path: dir.join("theme.json"),
            version: version_key("1.0.0"),
        }];
        assert_eq!(
            selected_in(Some(&selection), &themes).as_deref(),
            Some(SETI_THEME_ID)
        );
        std::fs::write(&selection, r#"{"id":null}"#).unwrap();
        assert_eq!(selected_in(Some(&selection), &themes), None);
        std::fs::write(&selection, r#"{"id":"custom"}"#).unwrap();
        assert_eq!(
            selected_in(Some(&selection), &themes).as_deref(),
            Some("custom")
        );
        assert_eq!(selected_in(None, &[]), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn refuses_paths_outside_extension() {
        let root = pipi_test_support::temp_dir("icon-theme");
        std::fs::write(root.join("icon.svg"), "<svg/>").unwrap();
        assert!(inside(&root, &root, "icon.svg").is_ok());
        assert!(inside(&root, &root, "../outside.svg").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("escape.svg")).unwrap();
            assert!(inside(&root, &root, "escape.svg").is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
