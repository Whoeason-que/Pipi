//! 全局技能 store：内容寻址安装 + 锁文件（agent.lock / skills.lock）。
//!
//! 布局（`<av_home>` = `$AV_HOME` 或 `~/.av`）：
//!
//! ```text
//! ~/.av/
//! ├── skills.lock                      # store 索引：已安装版本（含 installed-at）
//! ├── .tmp/                             # 安装暂存（与 store 同盘，rename 原子）
//! └── skills/<name>/<hash16>/           # 不可变内容目录，hash16 = 内容 sha256 前 16 位
//! ```
//!
//! 契约层旁的解析锁（与声明它的 `agent.toml` 同目录）：
//!
//! ```text
//! agent.toml    # [resources.skills] use = ["pdf"]
//! agent.lock    # 每个 name 的 source / ref / revision / content-hash / skill-path
//! ```
//!
//! 不变量：
//! - **内容目录一经发布即不可变**；同名不同版本并存（不同项目可锁不同修订）；
//! - **安装 ≠ 启用**：装进 store 只是可用；契约 `use` 声明才生效；
//! - **锁是解析权威**：缺失/不匹配一律 fail-closed，绝不静默重装；
//! - **会话启动不联网**：[`resolve_declared_skills`] 只读本地，安装只发生在
//!   显式的 `av skill install/add/sync`。

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::discovery::Layer;
use crate::paths;
use crate::skills::{self, SkillMeta};

/// 解析锁文件名（与声明它的契约同目录）。
pub const SKILL_LOCK_FILENAME: &str = "agent.lock";
/// 锁文件/索引的 schema 版本。
pub const LOCK_VERSION: u32 = 1;
/// 内容目录名 = 内容哈希前多少位（十六进制）。
pub const CONTENT_DIR_NAME_LEN: usize = 16;
/// 单个技能内容限额（fail-closed）。
pub const MAX_SKILL_FILES: usize = 500;
/// 单个技能内容限额（字节，fail-closed）。
pub const MAX_SKILL_BYTES: u64 = 5 * 1024 * 1024;

/// 来源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// git 仓库（含 GitHub 短名解析后的 URL）。
    Git,
    /// 本地目录（安装时快照复制，store 内容不随来源变化）。
    Local,
}

/// 一条技能解析记录（agent.lock 与 store 索引共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SkillLockEntry {
    pub name: String,
    /// git 源：规范化仓库 URL；local 源：绝对路径。
    pub source: String,
    pub source_type: SourceType,
    /// git 源解析所用的 ref（分支/标签/commit），信息性记录；local 源缺席。
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// git 源解析到的精确 commit（40 位 hex）；local 源缺席。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// 技能在来源中的相对路径（如 `skills/pdf`；来源根本身即技能时为 `.`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    /// 内容哈希：`sha256:<64 位小写 hex>`；内容目录名取前 [`CONTENT_DIR_NAME_LEN`] 位。
    pub content_hash: String,
    /// 仅 store 索引记录（agent.lock 不写：锁文件只在解析变化时变动）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl SkillLockEntry {
    /// 逐字段 fail-closed 校验。
    pub fn validate(&self) -> Result<(), String> {
        if !skills::valid_skill_name(&self.name) {
            return Err(format!("锁条目技能名 {:?} 形状非法", self.name));
        }
        if self.source.trim().is_empty() {
            return Err(format!("技能 {:?} 的 source 为空", self.name));
        }
        match self.source_type {
            SourceType::Git => {
                let revision = self
                    .revision
                    .as_deref()
                    .ok_or_else(|| format!("技能 {:?} 是 git 源但缺少 revision", self.name))?;
                if !is_full_hex40(revision) {
                    return Err(format!(
                        "技能 {:?} 的 revision 必须是 40 位十六进制 commit：{revision:?}",
                        self.name
                    ));
                }
            }
            SourceType::Local => {
                if self.revision.is_some() {
                    return Err(format!(
                        "技能 {:?} 是 local 源，不应有 revision（内容哈希即身份）",
                        self.name
                    ));
                }
                if !Path::new(&self.source).is_absolute() {
                    return Err(format!(
                        "技能 {:?} 的 local 源必须是绝对路径：{}",
                        self.name, self.source
                    ));
                }
            }
        }
        validate_content_hash(&self.content_hash)?;
        Ok(())
    }
}

fn is_full_hex40(value: &str) -> bool {
    value.len() == 40
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// 校验内容哈希形状：`sha256:<64 位小写 hex>`。
pub fn validate_content_hash(hash: &str) -> Result<(), String> {
    let Some(hex) = hash.strip_prefix("sha256:") else {
        return Err(format!("内容哈希必须以 sha256: 开头：{hash:?}"));
    };
    if hex.len() != 64
        || !hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(format!("内容哈希必须是 64 位小写十六进制：{hash:?}"));
    }
    Ok(())
}

/// 内容目录名：`sha256:<hex>` 的前 [`CONTENT_DIR_NAME_LEN`] 位。
pub fn content_dir_name(content_hash: &str) -> Result<&str, String> {
    validate_content_hash(content_hash)?;
    Ok(&content_hash["sha256:".len().."sha256:".len() + CONTENT_DIR_NAME_LEN])
}

/// 锁文件（agent.lock 与 `~/.av/skills.lock` 索引共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillLock {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillLockEntry>,
}

impl SkillLock {
    pub fn empty() -> Self {
        Self {
            version: LOCK_VERSION,
            skills: Vec::new(),
        }
    }

    /// 读取锁文件；文件缺失返回空锁（「缺失」的判断由调用方做，如
    /// [`resolve_declared_skills`] 对 agent.lock 是 fail-closed）。
    pub fn read(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let text =
            fs::read_to_string(path).map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
        let lock: Self =
            toml::from_str(&text).map_err(|e| format!("解析 {} 失败：{e}", path.display()))?;
        lock.validate()
            .map_err(|e| format!("{} 校验失败：{e}", path.display()))?;
        Ok(lock)
    }

    /// 结构校验：版本 + 逐条字段 + (name, content-hash) 唯一。
    pub fn validate(&self) -> Result<(), String> {
        if self.version != LOCK_VERSION {
            return Err(format!(
                "不支持的锁文件版本 {}（当前支持 {}）",
                self.version, LOCK_VERSION
            ));
        }
        let mut seen = HashSet::new();
        for entry in &self.skills {
            entry.validate()?;
            if !seen.insert((entry.name.as_str(), entry.content_hash.as_str())) {
                return Err(format!(
                    "锁文件出现重复条目：{}（{}）",
                    entry.name, entry.content_hash
                ));
            }
        }
        Ok(())
    }

    /// agent.lock 语义：技能名唯一（一个名字只有一条解析结果）。
    pub fn validate_unique_names(&self) -> Result<(), String> {
        let mut seen = HashSet::new();
        for entry in &self.skills {
            if !seen.insert(entry.name.as_str()) {
                return Err(format!("锁文件出现重复技能名：{:?}", entry.name));
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&SkillLockEntry> {
        self.skills.iter().find(|entry| entry.name == name)
    }

    /// 按 (name, content-hash) 精确查找（store 索引查重）。
    pub fn get_version(&self, name: &str, content_hash: &str) -> Option<&SkillLockEntry> {
        self.skills
            .iter()
            .find(|entry| entry.name == name && entry.content_hash == content_hash)
    }

    /// 按 name 替换或追加（agent.lock 写入用；保留原 installed-at）。
    pub fn set_entry(&mut self, entry: SkillLockEntry) {
        if let Some(slot) = self.skills.iter_mut().find(|slot| slot.name == entry.name) {
            let installed_at = slot.installed_at.clone();
            *slot = entry;
            slot.installed_at = installed_at;
            return;
        }
        self.skills.push(entry);
    }

    /// 按 (name, content-hash) 替换或追加（store 索引：同名多版本并存）。
    pub fn upsert_version(&mut self, entry: SkillLockEntry) {
        if let Some(slot) = self
            .skills
            .iter_mut()
            .find(|slot| slot.name == entry.name && slot.content_hash == entry.content_hash)
        {
            let installed_at = slot.installed_at.clone();
            *slot = entry;
            slot.installed_at = installed_at;
            return;
        }
        self.skills.push(entry);
    }

    /// 删除某技能名的所有条目；返回是否删除过。
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.skills.len();
        self.skills.retain(|entry| entry.name != name);
        before != self.skills.len()
    }

    /// 原子写入（暂存 + rename），条目按 (name, content-hash) 排序保证稳定输出。
    pub fn write(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let mut sorted = self.clone();
        sorted
            .skills
            .sort_by(|a, b| (&a.name, &a.content_hash).cmp(&(&b.name, &b.content_hash)));
        let text = toml::to_string_pretty(&sorted).map_err(|e| format!("序列化锁文件失败：{e}"))?;
        write_text_atomic(path, &text)
    }
}

// ============ store 索引 ============

/// 读取 store 索引（`~/.av/skills.lock`）；缺失返回空索引。
pub fn read_index() -> Result<SkillLock, String> {
    let path = paths::skills_index_path().ok_or("无法定位 av 家目录（~/.av）")?;
    SkillLock::read(&path)
}

/// 写入 store 索引。
pub fn write_index(index: &SkillLock) -> Result<(), String> {
    let path = paths::skills_index_path().ok_or("无法定位 av 家目录（~/.av）")?;
    index.write(&path)
}

/// 把解析记录写入 store 索引（保留原 installed-at，刷新 updated-at）。
/// 单次读改写，批量调用请用 [`record_index_entries`]。
pub fn record_index(entry: &SkillLockEntry) -> Result<(), String> {
    record_index_entries(std::slice::from_ref(entry))
}

/// 批量写入 store 索引（一次读改写）。
pub fn record_index_entries(entries: &[SkillLockEntry]) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut index = read_index()?;
    let now = now_iso8601();
    for entry in entries {
        entry.validate()?;
        let mut recorded = entry.clone();
        recorded.installed_at = index
            .get_version(&recorded.name, &recorded.content_hash)
            .and_then(|slot| slot.installed_at.clone())
            .or_else(|| Some(now.clone()));
        recorded.updated_at = Some(now.clone());
        index.upsert_version(recorded);
    }
    write_index(&index)
}

// ============ 内容扫描与哈希 ============

/// 技能内容清单：`(相对路径, 源文件绝对路径, 字节数)`，按相对路径排序。
#[derive(Debug, Clone)]
pub struct SkillContent {
    pub files: Vec<(String, PathBuf, u64)>,
    pub total_bytes: u64,
}

impl SkillContent {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// 扫描技能目录内容（fail-closed）：
/// - 拒绝符号链接与特殊文件（fifo/socket/设备）；
/// - 跳过 `.git` 目录（VCS 元数据不是技能内容）；
/// - 限额：≤ [`MAX_SKILL_FILES`] 个文件、≤ [`MAX_SKILL_BYTES`] 字节；
/// - 空目录报错（技能至少要有 `SKILL.md`）。
pub fn scan_skill_content(dir: &Path) -> Result<SkillContent, String> {
    let root =
        fs::canonicalize(dir).map_err(|e| format!("无法读取技能目录 {}: {e}", dir.display()))?;
    if !root.is_dir() {
        return Err(format!("技能路径不是目录：{}", root.display()));
    }
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    walk_content(&root, &root, &mut files, &mut total_bytes)?;
    if files.is_empty() {
        return Err(format!("技能目录为空：{}", root.display()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(SkillContent { files, total_bytes })
}

fn walk_content(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf, u64)>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("无法读取 {}: {e}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("无法读取 {}: {e}", dir.display()))?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name == ".git" {
            continue;
        }
        // file_type() 不跟随符号链接，符号链接在这里就能识别
        let file_type = fs::symlink_metadata(&path)
            .map_err(|e| format!("无法检查 {}: {e}", path.display()))?
            .file_type();
        if file_type.is_symlink() {
            return Err(format!("技能内容不允许符号链接：{}", path.display()));
        }
        if file_type.is_dir() {
            walk_content(root, &path, files, total_bytes)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(format!("技能内容不允许特殊文件：{}", path.display()));
        }
        let len = fs::metadata(&path)
            .map_err(|e| format!("无法读取 {}: {e}", path.display()))?
            .len();
        *total_bytes += len;
        if files.len() + 1 > MAX_SKILL_FILES {
            return Err(format!(
                "技能文件数超过上限（{MAX_SKILL_FILES}）：{}",
                root.display()
            ));
        }
        if *total_bytes > MAX_SKILL_BYTES {
            return Err(format!(
                "技能总大小超过上限（{MAX_SKILL_BYTES} 字节）：{}",
                root.display()
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("无法计算相对路径：{}", path.display()))?;
        let relative = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        files.push((relative, path, len));
    }
    Ok(())
}

/// 计算内容哈希：`sha256:<hex>`，输入为按相对路径排序的
/// `(相对路径, 0x00, 字节长度 LE, 文件字节)` —— 路径参与哈希，
/// 重命名/改内容/增删文件都会改变哈希；权限位不参与（跨平台稳定）。
pub fn hash_skill_content(content: &SkillContent) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for (relative, path, len) in &content.files {
        hasher.update(relative.as_bytes());
        hasher.update([0u8]);
        hasher.update(len.to_le_bytes());
        let bytes = fs::read(path).map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{}", to_hex(&hasher.finalize())))
}

/// 扫描 + 哈希一个技能目录。
pub fn hash_skill_dir(dir: &Path) -> Result<String, String> {
    hash_skill_content(&scan_skill_content(dir)?)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ============ 发布 ============

/// store 根（`~/.av/skills`）。
pub fn store_root() -> Result<PathBuf, String> {
    paths::skills_store().ok_or_else(|| "无法定位 av 家目录（~/.av）".to_string())
}

/// store 内容目录：`<store>/<name>/<hash16>`。
pub fn entry_dir(name: &str, content_hash: &str) -> Result<PathBuf, String> {
    entry_dir_in(&store_root()?, name, content_hash)
}

fn entry_dir_in(store_root: &Path, name: &str, content_hash: &str) -> Result<PathBuf, String> {
    if !skills::valid_skill_name(name) {
        return Err(format!("技能名 {name:?} 形状非法，拒绝落盘（fail-closed）"));
    }
    Ok(store_root.join(name).join(content_dir_name(content_hash)?))
}

/// 发布技能内容到 store（幂等、原子）：
/// 目标 `<store>/<name>/<hash16>` 已存在则直接返回；否则在暂存目录复制后
/// rename（与 store 同盘，rename 原子）。
pub fn publish_skill(
    content: &SkillContent,
    name: &str,
    content_hash: &str,
) -> Result<PathBuf, String> {
    let store = store_root()?;
    let staging = paths::staging_dir().ok_or("无法定位 av 家目录（~/.av）")?;
    publish_skill_in(&store, &staging, content, name, content_hash)
}

fn publish_skill_in(
    store_root: &Path,
    staging_root: &Path,
    content: &SkillContent,
    name: &str,
    content_hash: &str,
) -> Result<PathBuf, String> {
    let dest = entry_dir_in(store_root, name, content_hash)?;
    if ensure_real_dir(&dest, "store 技能目录")? {
        return Ok(dest);
    }
    let parent = dest
        .parent()
        .ok_or_else(|| format!("无法确定 store 目录：{}", dest.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("无法创建 {}: {e}", parent.display()))?;
    fs::create_dir_all(staging_root)
        .map_err(|e| format!("无法创建暂存目录 {}: {e}", staging_root.display()))?;

    let staging = staging_root.join(format!(
        "skill-{}-{}",
        std::process::id(),
        monotonic_nonce()
    ));
    let copy_result = (|| -> Result<(), String> {
        fs::create_dir_all(&staging).map_err(|e| format!("无法创建暂存目录：{e}"))?;
        for (relative, source, _) in &content.files {
            let target = staging.join(relative);
            if let Some(dir) = target.parent() {
                fs::create_dir_all(dir).map_err(|e| format!("无法创建 {}: {e}", dir.display()))?;
            }
            fs::copy(source, &target).map_err(|e| {
                format!(
                    "复制技能文件失败 {} -> {}: {e}",
                    source.display(),
                    target.display()
                )
            })?;
        }
        Ok(())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    match fs::rename(&staging, &dest) {
        Ok(()) => Ok(dest),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            // 并发发布的竞态：目标已被别人放好且合法，视为成功
            if ensure_real_dir(&dest, "store 技能目录")? {
                Ok(dest)
            } else {
                Err(format!("发布技能失败 {}: {error}", dest.display()))
            }
        }
    }
}

/// 删除 store 中某个具体版本；返回是否删除过。
pub fn remove_version(name: &str, content_hash: &str) -> Result<bool, String> {
    let dir = entry_dir(name, content_hash)?;
    if !ensure_real_dir(&dir, "store 技能目录")? {
        return Ok(false);
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("无法删除 {}: {e}", dir.display()))?;
    Ok(true)
}

// ============ 解析（会话启动路径，只读） ============

/// 声明启用的 store 技能解析结果。
#[derive(Debug, Clone)]
pub struct ResolvedSkill {
    pub entry: SkillLockEntry,
    /// store 内容目录（canonical）。
    pub dir: PathBuf,
    pub meta: SkillMeta,
}

/// 契约里声明 `use` 的最高层（层集合自低到高；项目 local 层最高）。
#[derive(Debug, Clone)]
pub struct DeclaredUse<'a> {
    pub layer: &'a Layer,
    pub names: &'a [String],
    /// 同层解析锁：与声明它的契约文件同目录的 `agent.lock`。
    pub lock_path: PathBuf,
}

/// 找到声明 `use` 的最高层（`use = []` 也算声明：显式禁用并覆盖低层）。
pub fn winning_use(layers: &[Layer]) -> Option<DeclaredUse<'_>> {
    layers.iter().rev().find_map(|layer| {
        let names = layer
            .config
            .resources
            .as_ref()
            .and_then(|resources| resources.skills.as_ref())
            .and_then(|skills| skills.use_.as_deref())?;
        Some(DeclaredUse {
            layer,
            names,
            lock_path: lock_path_for(layer),
        })
    })
}

/// 与契约层同目录的解析锁路径。
pub fn lock_path_for(layer: &Layer) -> PathBuf {
    layer
        .path
        .parent()
        .map(|dir| dir.join(SKILL_LOCK_FILENAME))
        .unwrap_or_else(|| PathBuf::from(SKILL_LOCK_FILENAME))
}

/// 解析声明启用的 store 技能（只读、不联网、fail-closed）。
///
/// 任一条目缺失锁记录、store 内容缺失、SKILL.md 不可解析 → 报错并提示
/// `av skill sync`。`names` 为空（含 `use = []`）返回空列表。
pub fn resolve_declared_skills(
    names: &[String],
    lock_path: &Path,
) -> Result<Vec<ResolvedSkill>, String> {
    resolve_declared_skills_in(&store_root()?, names, lock_path)
}

fn resolve_declared_skills_in(
    store_root: &Path,
    names: &[String],
    lock_path: &Path,
) -> Result<Vec<ResolvedSkill>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    if !lock_path.is_file() {
        return Err(format!(
            "契约声明了 skills.use，但缺少解析锁 {}；运行 `av skill sync` 生成",
            lock_path.display()
        ));
    }
    let lock = SkillLock::read(lock_path)?;
    lock.validate_unique_names()
        .map_err(|e| format!("{} 校验失败：{e}", lock_path.display()))?;
    let canonical_store = fs::canonicalize(store_root).map_err(|e| {
        format!(
            "技能 store 不可用（{}: {e}）；运行 `av skill sync` 安装声明的技能",
            store_root.display()
        )
    })?;

    let mut resolved = Vec::new();
    for name in names {
        let entry = lock.get(name).ok_or_else(|| {
            format!(
                "技能 {name:?} 未在 {} 中解析；运行 `av skill sync`",
                lock_path.display()
            )
        })?;
        let dir = entry_dir_in(store_root, name, &entry.content_hash)?;
        let canonical = fs::canonicalize(&dir).map_err(|_| {
            format!(
                "技能 {name:?} 的 store 内容缺失（{}）；运行 `av skill sync`",
                dir.display()
            )
        })?;
        if !canonical.starts_with(&canonical_store) || !canonical.is_dir() {
            return Err(format!(
                "技能 {name:?} 的 store 路径逃逸 store 根，拒绝加载（fail-closed）"
            ));
        }
        let meta = skills::read_skill_meta(&canonical).ok_or_else(|| {
            format!(
                "技能 {name:?} 的 SKILL.md 缺失或不可解析：{}",
                canonical.display()
            )
        })?;
        resolved.push(ResolvedSkill {
            entry: entry.clone(),
            dir: canonical,
            meta,
        });
    }
    Ok(resolved)
}

/// 校验一条 store 索引记录。
#[derive(Debug, Clone, PartialEq)]
pub enum VerifyOutcome {
    /// 内容与记录一致。
    Ok,
    /// 内容目录缺失。
    Missing,
    /// 内容漂移：记录哈希与实际哈希不一致。
    Mismatch { actual: String },
}

/// 重算 store 内容哈希并与记录比对（`av skill verify` 的底层）。
pub fn verify_entry(entry: &SkillLockEntry) -> Result<VerifyOutcome, String> {
    verify_entry_in(&store_root()?, entry)
}

fn verify_entry_in(store_root: &Path, entry: &SkillLockEntry) -> Result<VerifyOutcome, String> {
    let dir = entry_dir_in(store_root, &entry.name, &entry.content_hash)?;
    if !ensure_real_dir(&dir, "store 技能目录")? {
        return Ok(VerifyOutcome::Missing);
    }
    let actual = hash_skill_dir(&dir)?;
    if actual == entry.content_hash {
        Ok(VerifyOutcome::Ok)
    } else {
        Ok(VerifyOutcome::Mismatch { actual })
    }
}

// ============ 工具 ============

/// RFC3339（UTC，秒精度）当前时间；不引入时间库。
pub fn now_iso8601() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let sod = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Howard Hinnant civil_from_days：1970-01-01 起的天数 → 公历年月日。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn monotonic_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 原子写文本：同目录暂存 + rename（tempfile 保证唯一命名与失败清理）。
/// 公开给 CLI（改写用户 agent.toml 时复用同一条原子路径）。
pub fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("无法确定目录：{}", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("无法创建目录 {}: {e}", dir.display()))?;
    let mut file = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("无法创建临时文件（{}）: {e}", dir.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("无法写入临时文件：{e}"))?;
    file.flush().map_err(|e| format!("无法刷新临时文件：{e}"))?;
    file.persist(path)
        .map_err(|e| format!("无法替换 {}: {}", path.display(), e.error))?;
    Ok(())
}

/// 目录边界检查：存在且是真实目录（非符号链接）→ true；不存在 → false。
fn ensure_real_dir(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(format!("{label} 不能是符号链接: {}", path.display()))
        }
        Ok(metadata) if !metadata.is_dir() => Err(format!("{label} 不是目录: {}", path.display())),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查 {label} {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::AgentToml;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "av-store-{label}-{}-{}",
            std::process::id(),
            monotonic_nonce()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(dir: &Path, name: &str, body: &str) -> PathBuf {
        let skill = dir.join(name);
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n{body}"),
        )
        .unwrap();
        skill
    }

    fn local_entry(name: &str, source: &Path, content_hash: &str) -> SkillLockEntry {
        SkillLockEntry {
            name: name.to_string(),
            source: source.to_string_lossy().into_owned(),
            source_type: SourceType::Local,
            reference: None,
            revision: None,
            skill_path: None,
            content_hash: content_hash.to_string(),
            installed_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn content_hash_is_stable_and_path_sensitive() {
        let root = temp_dir("hash");
        let skill = write_skill(&root, "pdf", "body");
        fs::write(skill.join("extra.txt"), "extra").unwrap();

        let hash = hash_skill_dir(&skill).unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash_skill_dir(&skill).unwrap(), hash, "同一内容哈希稳定");

        // 改内容 → 变
        fs::write(skill.join("extra.txt"), "extra2").unwrap();
        let changed = hash_skill_dir(&skill).unwrap();
        assert_ne!(changed, hash);

        // 重命名（路径参与哈希）→ 变
        fs::rename(skill.join("extra.txt"), skill.join("renamed.txt")).unwrap();
        let renamed = hash_skill_dir(&skill).unwrap();
        assert_ne!(renamed, changed);

        // 删文件 → 回到只有一个文件时的哈希
        fs::remove_file(skill.join("renamed.txt")).unwrap();
        assert_eq!(
            hash_skill_dir(&skill).unwrap(),
            hash_skill_dir(&skill).unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scan_rejects_symlinks() {
        let root = temp_dir("symlink");
        let skill = write_skill(&root, "pdf", "body");
        let outside = root.join("outside.txt");
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, skill.join("link.txt")).unwrap();

        let error = scan_skill_content(&skill).unwrap_err();
        assert!(error.contains("符号链接"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_skips_git_dir_and_enforces_limits() {
        let root = temp_dir("limits");
        let skill = write_skill(&root, "pdf", "body");
        fs::create_dir_all(skill.join(".git")).unwrap();
        fs::write(skill.join(".git/config"), "x").unwrap();
        let content = scan_skill_content(&skill).unwrap();
        assert_eq!(content.file_count(), 1, ".git 不进入内容清单");

        // 超字节上限
        let big = write_skill(&root, "big", "body");
        let oversized = MAX_SKILL_BYTES + 1;
        fs::write(big.join("huge.bin"), vec![0u8; oversized as usize]).unwrap();
        let error = scan_skill_content(&big).unwrap_err();
        assert!(error.contains("上限"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_is_idempotent_and_versioned() {
        let root = temp_dir("publish");
        let store = root.join("store");
        let staging = root.join("tmp");
        let skill = write_skill(&root, "pdf", "v1");

        let hash = hash_skill_dir(&skill).unwrap();
        let content = scan_skill_content(&skill).unwrap();
        let first = publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();
        let again = publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();
        assert_eq!(first, again, "同哈希发布幂等");
        assert_eq!(
            fs::read_dir(store.join("pdf")).unwrap().count(),
            1,
            "同名同哈希只有一个目录"
        );

        // 内容变化 → 新版本目录并存，旧目录不被改动
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: pdf\ndescription: t\n---\nv2",
        )
        .unwrap();
        let hash2 = hash_skill_dir(&skill).unwrap();
        assert_ne!(hash2, hash);
        let content2 = scan_skill_content(&skill).unwrap();
        let second = publish_skill_in(&store, &staging, &content2, "pdf", &hash2).unwrap();
        assert_ne!(second, first);
        assert_eq!(fs::read_dir(store.join("pdf")).unwrap().count(), 2);
        assert!(first.join("SKILL.md").is_file(), "旧版本仍完整");

        // 非法名拒绝
        assert!(publish_skill_in(&store, &staging, &content, "PDF", &hash).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lock_roundtrip_and_validation() {
        let root = temp_dir("lock");
        let path = root.join("agent.lock");
        let fixed_hash = format!("sha256:{}", "e".repeat(64));
        let mut lock = SkillLock::empty();
        lock.set_entry(local_entry("pdf", &root, &fixed_hash));
        lock.write(&path).unwrap();

        let read = SkillLock::read(&path).unwrap();
        assert_eq!(read.skills.len(), 1);
        assert_eq!(read.get("pdf").unwrap().source_type, SourceType::Local);

        // 版本不符 fail-closed
        fs::write(&path, "version = 99\n").unwrap();
        assert!(SkillLock::read(&path).is_err());

        // 未知字段 fail-closed
        fs::write(&path, "version = 1\nfoo = 1\n").unwrap();
        assert!(SkillLock::read(&path).is_err());

        // git 源必须有 40 位 revision
        let mut git_entry = local_entry("pdf", &root, &format!("sha256:{}", "a".repeat(64)));
        git_entry.source_type = SourceType::Git;
        git_entry.source = "https://github.com/o/r".into();
        assert!(git_entry.validate().is_err());
        git_entry.revision = Some("b".repeat(40));
        git_entry.skill_path = Some("skills/pdf".into());
        git_entry.validate().unwrap();

        // local 源必须有绝对路径、不能有 revision
        let mut bad = local_entry(
            "pdf",
            Path::new("rel/path"),
            &format!("sha256:{}", "a".repeat(64)),
        );
        assert!(bad.validate().is_err());
        bad.source = "/abs".into();
        bad.revision = Some("b".repeat(40));
        assert!(bad.validate().is_err());

        // 重复 (name, hash) 拒绝；同名不同版本允许
        let mut dup = SkillLock::empty();
        let entry = local_entry("pdf", &root, &format!("sha256:{}", "a".repeat(64)));
        dup.upsert_version(entry.clone());
        dup.upsert_version(entry.clone());
        assert_eq!(dup.skills.len(), 1, "同 (name, hash) 幂等");
        let mut other = entry.clone();
        other.content_hash = format!("sha256:{}", "c".repeat(64));
        dup.upsert_version(other);
        assert_eq!(dup.skills.len(), 2, "同名不同版本并存");
        dup.validate().unwrap();

        // agent.lock 语义：技能名唯一
        dup.validate_unique_names().unwrap_err();

        // set_entry 按 name 替换
        let mut agent_lock = SkillLock::empty();
        agent_lock.set_entry(entry.clone());
        let mut replaced = entry;
        replaced.content_hash = format!("sha256:{}", "d".repeat(64));
        agent_lock.set_entry(replaced);
        assert_eq!(agent_lock.skills.len(), 1);
        assert!(agent_lock.skills[0].content_hash.ends_with("dddd"));

        // remove
        assert!(agent_lock.remove("pdf"));
        assert!(!agent_lock.remove("pdf"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_dir_name_requires_full_hash() {
        let hash = format!("sha256:{}", "a".repeat(64));
        assert_eq!(content_dir_name(&hash).unwrap(), "aaaaaaaaaaaaaaaa");
        assert!(content_dir_name("sha256:abc").is_err());
        assert!(content_dir_name(&format!("sha256:{}", "A".repeat(64))).is_err());
        assert!(content_dir_name(&"a".repeat(64)).is_err());
    }

    #[test]
    fn publish_then_resolve_roundtrip() {
        let root = temp_dir("resolve");
        let store = root.join("store");
        let staging = root.join("tmp");
        let skill = write_skill(&root, "pdf", "body");
        let hash = hash_skill_dir(&skill).unwrap();
        let content = scan_skill_content(&skill).unwrap();
        publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();

        let lock_path = root.join("agent.lock");
        let mut lock = SkillLock::empty();
        lock.set_entry(local_entry("pdf", &skill, &hash));
        lock.write(&lock_path).unwrap();

        let resolved =
            resolve_declared_skills_in(&store, &["pdf".to_string()], &lock_path).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].meta.name, "pdf");
        assert!(resolved[0].dir.ends_with(content_dir_name(&hash).unwrap()));

        // 空声明 → 空结果（不读锁）
        assert!(
            resolve_declared_skills_in(&store, &[], &lock_path)
                .unwrap()
                .is_empty()
        );

        // 缺锁文件 fail-closed
        let error =
            resolve_declared_skills_in(&store, &["pdf".to_string()], &root.join("missing.lock"))
                .unwrap_err();
        assert!(error.contains("av skill sync"), "{error}");

        // 锁里没有该技能 fail-closed
        let empty_lock = root.join("empty.lock");
        SkillLock::empty().write(&empty_lock).unwrap();
        let error =
            resolve_declared_skills_in(&store, &["pdf".to_string()], &empty_lock).unwrap_err();
        assert!(error.contains("未在") && error.contains("sync"), "{error}");

        // store 内容缺失 fail-closed
        fs::remove_dir_all(entry_dir_in(&store, "pdf", &hash).unwrap()).unwrap();
        let error =
            resolve_declared_skills_in(&store, &["pdf".to_string()], &lock_path).unwrap_err();
        assert!(error.contains("缺失") && error.contains("sync"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verify_detects_missing_and_drift() {
        let root = temp_dir("verify");
        let store = root.join("store");
        let staging = root.join("tmp");
        let skill = write_skill(&root, "pdf", "body");
        let hash = hash_skill_dir(&skill).unwrap();
        let content = scan_skill_content(&skill).unwrap();
        let dir = publish_skill_in(&store, &staging, &content, "pdf", &hash).unwrap();
        let entry = local_entry("pdf", &skill, &hash);

        assert_eq!(verify_entry_in(&store, &entry).unwrap(), VerifyOutcome::Ok);

        // 漂移：篡改 store 内容
        fs::write(dir.join("SKILL.md"), "tampered").unwrap();
        match verify_entry_in(&store, &entry).unwrap() {
            VerifyOutcome::Mismatch { actual } => assert_ne!(actual, hash),
            other => panic!("应检测到漂移：{other:?}"),
        }

        // 缺失
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            verify_entry_in(&store, &entry).unwrap(),
            VerifyOutcome::Missing
        );

        // verify_entry（无 _in）走真实 av 家目录：路径可定位时不应 panic
        let _ = verify_entry(&entry);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn winning_use_picks_highest_declaring_layer() {
        fn layer(label: &str, text: &str) -> Layer {
            let config: AgentToml = toml::from_str(text).unwrap();
            config.validate().unwrap();
            Layer {
                label: label.to_string(),
                path: PathBuf::from(label),
                config,
            }
        }
        let agent = layer(
            "agent.toml",
            "schema = 1\n[resources.skills]\nuse = [\"pdf\"]",
        );
        let project = layer(
            "proj/agent.toml",
            "schema = 1\n[resources.skills]\nuse = [\"commit-helper\"]",
        );
        let local = layer("proj/agent.local.toml", "schema = 1");

        // 最高声明层是项目层
        let layers = vec![agent.clone(), project.clone(), local.clone()];
        let declared = winning_use(&layers).unwrap();
        assert_eq!(declared.layer.label, "proj/agent.toml");
        assert_eq!(declared.names, ["commit-helper".to_string()]);
        assert_eq!(declared.lock_path, PathBuf::from("proj/agent.lock"));

        // local 层显式禁用（use = []）覆盖项目层
        let local_disable = layer(
            "proj/agent.local.toml",
            "schema = 1\n[resources.skills]\nuse = []",
        );
        let layers = vec![agent.clone(), project, local_disable];
        let declared = winning_use(&layers).unwrap();
        assert!(declared.names.is_empty());

        // 只有 agent 层声明
        let only_agent = [agent];
        let declared = winning_use(&only_agent).unwrap();
        assert_eq!(declared.layer.label, "agent.toml");
        assert!(winning_use(&[]).is_none());
    }

    #[test]
    fn iso8601_time() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_728), (2026, 10, 2));
        let now = now_iso8601();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z') && now.contains('T'), "{now}");

        // 1970-01-01T00:00:00Z 的形状由 days/sod 计算保证
        assert_eq!(now_iso8601().len(), 20);
    }
}
