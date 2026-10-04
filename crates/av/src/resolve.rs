//! 环境解析：进程环境 + 声明层 + 运行时注入 → 最终子进程环境。
//!
//! 拼装顺序（确定性）：
//! 1. 进程环境按继承策略（`all` / `core` / `none`）取入；
//! 2. `ignore` 只删除继承来的键，不触及声明值；
//! 3. `set` 覆盖/新增；
//! 4. `path-prepend` 前置到 PATH；
//! 5. `secrets` 引用解析（env 引用从**原始**进程环境取值，绕过 inherit/ignore）；
//! 6. 运行时注入（`AV_*`）最后且不可被声明覆盖。
//!
//! 解析一次、会话内共用：本函数在会话启动时调用一次，之后所有工具子进程
//! 使用同一份结果 —— 中途改契约文件不生效（pin 语义）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::discovery::Layer;
use crate::merge::merge_layers;
use crate::schema::{Inherit, SecretRef, validate_env_key, validate_env_value};

/// 进程环境变量的来源标签。
pub const PROCESS_SOURCE: &str = "process";
/// 运行时注入变量的来源标签。
pub const RUNTIME_SOURCE: &str = "runtime";

/// `inherit = "core"` 时的最小安全集。
const CORE_INHERIT_KEYS: &[&str] = &[
    "PATH", "HOME", "SHELL", "USER", "LOGNAME", "TERM", "LANG", "LC_ALL", "TZ", "TMPDIR",
];

/// 解析结果：最终环境 + 来源记账。
#[derive(Debug, Clone, Default)]
pub struct ResolvedEnv {
    /// 最终键值（完整子进程环境；BTreeMap 保证输出顺序稳定）。
    pub vars: BTreeMap<String, String>,
    /// 键 → 来源层标签（"process" / 契约层 label / "runtime"）。
    pub provenance: BTreeMap<String, String>,
    /// 值来自秘密引用的键（输出/落盘时必须脱敏）。
    pub secret_keys: BTreeSet<String>,
}

impl ResolvedEnv {
    /// 输出用键值对：秘密值替换为 `[redacted]`，保证永不泄入日志/会话记录。
    pub fn redacted(&self) -> BTreeMap<String, String> {
        self.vars
            .iter()
            .map(|(key, value)| {
                let value = if self.secret_keys.contains(key) {
                    "[redacted]".to_string()
                } else {
                    value.clone()
                };
                (key.clone(), value)
            })
            .collect()
    }
}

/// 收集当前进程环境（非 UTF-8 键值按 lossy 处理，不 panic）。
pub fn collect_process_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// 解析最终子进程环境。
///
/// `layers` 自低到高优先级；`process_env` 是原始进程环境；`runtime_vars`
/// 是宿主注入的 `AV_*` 变量（最后写入，覆盖一切）。
pub fn resolve_env(
    layers: &[Layer],
    process_env: &BTreeMap<String, String>,
    runtime_vars: &BTreeMap<String, String>,
) -> Result<ResolvedEnv, String> {
    let merged = merge_layers(layers)?;
    for key in runtime_vars.keys() {
        validate_env_key(key)?;
        validate_env_value(runtime_vars.get(key).expect("key exists"))?;
    }

    // 1) 继承策略
    let mut vars: BTreeMap<String, String> = match merged.env.inherit {
        Inherit::All => process_env.clone(),
        Inherit::Core => CORE_INHERIT_KEYS
            .iter()
            .filter_map(|key| {
                process_env
                    .get(*key)
                    .map(|v| ((*key).to_string(), v.clone()))
            })
            .collect(),
        Inherit::None => BTreeMap::new(),
    };
    let mut provenance = vars
        .keys()
        .map(|key| (key.clone(), PROCESS_SOURCE.to_string()))
        .collect::<BTreeMap<_, _>>();

    // 2) ignore：只作用于继承来的键
    let patterns = merged
        .env
        .ignore
        .iter()
        .map(|pattern| {
            glob::Pattern::new(pattern).map_err(|e| format!("ignore 模式 {pattern:?} 无效：{e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ignored = vars
        .keys()
        .filter(|key| patterns.iter().any(|pattern| pattern.matches(key)))
        .cloned()
        .collect::<Vec<_>>();
    for key in ignored {
        vars.remove(&key);
        provenance.remove(&key);
    }

    // 3) set 覆盖（来源 = 最后声明该键的层）
    for (key, value) in &merged.env.set {
        validate_env_value(value)?;
        vars.insert(key.clone(), value.clone());
        let source = owning_layer(layers, |config| {
            config
                .env
                .as_ref()
                .and_then(|env| env.set.as_ref())
                .is_some_and(|set| set.contains_key(key))
        })
        .map(|layer| layer.label.clone())
        .unwrap_or_else(|| PROCESS_SOURCE.to_string());
        provenance.insert(key.clone(), source);
    }

    // 4) path-prepend：相对路径以最后声明该列表的契约层目录为基准
    if !merged.env.path_prepend.is_empty() {
        let owner_dir = layers
            .iter()
            .rev()
            .find(|layer| {
                layer
                    .config
                    .env
                    .as_ref()
                    .and_then(|env| env.path_prepend.as_ref())
                    .is_some()
            })
            .and_then(|layer| layer.path.parent().map(Path::to_path_buf));
        let mut paths: Vec<PathBuf> = merged
            .env
            .path_prepend
            .iter()
            .map(|raw| match &owner_dir {
                Some(base) => crate::discovery::resolve_path(base, raw),
                None => crate::discovery::expand_tilde(raw),
            })
            .collect();
        if let Some(existing) = vars.get("PATH") {
            paths.extend(std::env::split_paths(existing));
        }
        let joined =
            std::env::join_paths(&paths).map_err(|e| format!("path-prepend 合并失败：{e}"))?;
        vars.insert("PATH".to_string(), joined.to_string_lossy().into_owned());
    }

    // 5) secrets：env 引用从原始进程环境取值（绕过 inherit/ignore）
    for (key, reference) in &merged.env.secrets {
        let (value, source) = match reference {
            SecretRef::Env(name) => {
                validate_env_key(name)?;
                let value = process_env
                    .get(name)
                    .ok_or_else(|| format!("秘密引用失败：进程环境不存在 {name:?}（{key}）"))?;
                if value.is_empty() {
                    return Err(format!("秘密引用失败：{name:?} 的值为空"));
                }
                (value.clone(), PROCESS_SOURCE.to_string())
            }
            SecretRef::File(raw) => {
                let owner = owning_layer(layers, |config| {
                    config
                        .env
                        .as_ref()
                        .and_then(|env| env.secrets.as_ref())
                        .is_some_and(|secrets| secrets.contains_key(key))
                });
                let path = owner
                    .as_ref()
                    .and_then(|layer| layer.path.parent().map(Path::to_path_buf))
                    .map(|base| crate::discovery::resolve_path(&base, raw))
                    .unwrap_or_else(|| crate::discovery::expand_tilde(raw));
                let label = owner
                    .map(|layer| layer.label.clone())
                    .unwrap_or_else(|| PROCESS_SOURCE.to_string());
                (read_secret_file(&path)?, label)
            }
        };
        validate_env_value(&value)?;
        vars.insert(key.clone(), value);
        provenance.insert(key.clone(), source);
    }

    // 6) 运行时注入：最后写入，不可被声明覆盖
    for (key, value) in runtime_vars {
        vars.insert(key.clone(), value.clone());
        provenance.insert(key.clone(), RUNTIME_SOURCE.to_string());
    }

    let secret_keys = merged.env.secrets.keys().cloned().collect::<BTreeSet<_>>();

    Ok(ResolvedEnv {
        vars,
        provenance,
        secret_keys,
    })
}

fn read_secret_file(path: &Path) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)
            .map_err(|e| format!("无法读取秘密文件 {}: {e}", path.display()))?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "秘密文件权限过宽（{:o}）：仅属主可读（0600）：{}",
                mode,
                path.display()
            ));
        }
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取秘密文件 {}: {e}", path.display()))?;
    let value = content.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        return Err(format!("秘密文件为空：{}", path.display()));
    }
    Ok(value)
}

/// 最后一个满足谓词的契约层（来源记账用：同键多层声明时取高层）。
fn owning_layer(
    layers: &[Layer],
    matches: impl Fn(&crate::schema::AgentToml) -> bool,
) -> Option<&Layer> {
    layers.iter().rev().find(|layer| matches(&layer.config))
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
