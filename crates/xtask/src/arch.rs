//! `cargo xtask arch` 的实现：读 `architecture.toml` + 各成员的 Cargo.toml，
//! 校验内部依赖白名单、禁用依赖、无环与登记完整性。
//!
//! 检查逻辑是纯函数（[`check`]），文件读取在 [`run`] 里，测试用合成数据喂
//! 反例，不依赖真实工作区。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

/// 根规则文件。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    crates: BTreeMap<String, CrateRule>,
}

/// 单个 crate 的规则。`may_depend_on` 是内部依赖白名单；`forbid` 点名第三方。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct CrateRule {
    #[serde(default)]
    may_depend_on: Vec<String>,
    #[serde(default)]
    dev_may_depend_on: Vec<String>,
    #[serde(default)]
    forbid: Vec<String>,
}

/// 一个成员的实际依赖（只取名字）。
#[derive(Debug, Default, Clone)]
struct CrateDeps {
    /// `[dependencies]` + `[target.*.dependencies]`。
    deps: BTreeSet<String>,
    /// `[dev-dependencies]` + `[target.*.dev-dependencies]`。
    dev_deps: BTreeSet<String>,
    /// `[build-dependencies]`（一律视为生产依赖）。
    build_deps: BTreeSet<String>,
}

impl CrateDeps {
    fn all_production(&self) -> BTreeSet<&str> {
        self.deps
            .iter()
            .chain(self.build_deps.iter())
            .map(String::as_str)
            .collect()
    }
}

/// 一条违规。
#[derive(Debug, PartialEq, Eq)]
enum Violation {
    /// 工作区成员没有在 architecture.toml 登记。
    Unregistered(String),
    /// 登记了但不是工作区成员（拼错或已删除）。
    UnknownCrate(String),
    /// 内部依赖不在白名单里。
    Undeclared {
        krate: String,
        dep: String,
        dev: bool,
    },
    /// 命中禁用依赖（第三方或内部）。
    Forbidden { krate: String, dep: String },
    /// 生产依赖图里有环。
    Cycle(Vec<String>),
}

impl Violation {
    fn render(&self) -> String {
        match self {
            Self::Unregistered(krate) => format!(
                "{krate}: 工作区成员未在 architecture.toml 登记（新增 crate 必须登记依赖白名单）"
            ),
            Self::UnknownCrate(krate) => {
                format!("{krate}: architecture.toml 里的登记不对应任何工作区成员")
            }
            Self::Undeclared { krate, dep, dev } => {
                let kind = if *dev {
                    "dev-dependencies"
                } else {
                    "dependencies"
                };
                format!("{krate}: {kind} 依赖 {dep}，但白名单未允许")
            }
            Self::Forbidden { krate, dep } => {
                format!("{krate}: 生产依赖命中 {dep}（见 architecture.toml 的 forbid）")
            }
            Self::Cycle(path) => format!("依赖环: {}", path.join(" -> ")),
        }
    }
}

/// 纯检查：规则 + 各成员实际依赖 → 违规列表。
fn check(
    rules: &BTreeMap<String, CrateRule>,
    crates: &BTreeMap<String, CrateDeps>,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    let members: BTreeSet<String> = crates.keys().cloned().collect();

    for name in &members {
        if !rules.contains_key(name) {
            violations.push(Violation::Unregistered(name.clone()));
        }
    }
    for name in rules.keys() {
        if !members.contains(name) {
            violations.push(Violation::UnknownCrate(name.clone()));
        }
    }

    for (name, rule) in rules {
        let Some(deps) = crates.get(name) else {
            continue;
        };
        let allowed: BTreeSet<&str> = rule.may_depend_on.iter().map(String::as_str).collect();
        let dev_allowed: BTreeSet<&str> =
            rule.dev_may_depend_on.iter().map(String::as_str).collect();
        let forbidden: BTreeSet<&str> = rule.forbid.iter().map(String::as_str).collect();

        for dep in deps.all_production() {
            if members.contains(dep) && !allowed.contains(dep) {
                violations.push(Violation::Undeclared {
                    krate: name.clone(),
                    dep: dep.to_string(),
                    dev: false,
                });
            }
            if forbidden.contains(dep) {
                violations.push(Violation::Forbidden {
                    krate: name.clone(),
                    dep: dep.to_string(),
                });
            }
        }
        for dep in &deps.dev_deps {
            if members.contains(dep) && !dev_allowed.contains(dep.as_str()) {
                violations.push(Violation::Undeclared {
                    krate: name.clone(),
                    dep: dep.clone(),
                    dev: true,
                });
            }
        }
    }

    if let Some(cycle) = find_cycle(rules, crates) {
        violations.push(Violation::Cycle(cycle));
    }
    violations
}

/// 在生产依赖图（只含内部边）上找一个环。白名单本身是 DAG 时无环，但依赖
/// 白名单写错（互相允许）就会在这里暴露。
fn find_cycle(
    rules: &BTreeMap<String, CrateRule>,
    crates: &BTreeMap<String, CrateDeps>,
) -> Option<Vec<String>> {
    let members: BTreeSet<&str> = crates.keys().map(String::as_str).collect();
    let mut state: BTreeMap<String, u8> = BTreeMap::new(); // 0 未访问 / 1 在栈上 / 2 完成
    let mut stack: Vec<String> = Vec::new();

    fn visit(
        node: &str,
        rules: &BTreeMap<String, CrateRule>,
        crates: &BTreeMap<String, CrateDeps>,
        members: &BTreeSet<&str>,
        state: &mut BTreeMap<String, u8>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        match state.get(node) {
            Some(1) => {
                let start = stack.iter().position(|item| item == node).unwrap_or(0);
                let mut path = stack[start..].to_vec();
                path.push(node.to_string());
                return Some(path);
            }
            Some(_) => return None,
            None => {}
        }
        state.insert(node.to_string(), 1);
        stack.push(node.to_string());

        let allowed: BTreeSet<&str> = rules
            .get(node)
            .map(|rule| rule.may_depend_on.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let deps: Vec<String> = crates
            .get(node)
            .map(|deps| {
                deps.all_production()
                    .into_iter()
                    .filter(|dep| members.contains(dep))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for dep in deps {
            // 只沿着「规则允许的边」走环：未声明的边已经单独报违规。
            if !allowed.contains(dep.as_str()) {
                continue;
            }
            if let Some(cycle) = visit(&dep, rules, crates, members, state, stack) {
                return Some(cycle);
            }
        }
        stack.pop();
        state.insert(node.to_string(), 2);
        None
    }

    for name in crates.keys() {
        if let Some(cycle) = visit(name, rules, crates, &members, &mut state, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

/// 成员目录（相对工作区根）→ 包名与实际依赖。
#[derive(Debug, Deserialize)]
struct MemberManifest {
    package: MemberPackage,
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    target: BTreeMap<String, TargetManifest>,
}

#[derive(Debug, Deserialize)]
struct MemberPackage {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct TargetManifest {
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceManifest {
    workspace: WorkspaceSection,
}

#[derive(Debug, Deserialize)]
struct WorkspaceSection {
    members: Vec<String>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask 位于 <root>/crates/xtask")
        .to_path_buf()
}

fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    toml::from_str(&text).map_err(|error| format!("解析 {} 失败: {error}", path.display()))
}

/// 入口：读规则与成员清单，跑检查，打印结果。
pub fn run() -> ExitCode {
    let root = workspace_root();
    let rules_path = root.join("architecture.toml");
    let rules_file: RulesFile = match read_toml(&rules_path) {
        Ok(rules) => rules,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let workspace: WorkspaceManifest = match read_toml(&root.join("Cargo.toml")) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let mut crates = BTreeMap::new();
    for member in &workspace.workspace.members {
        let manifest_path = root.join(member).join("Cargo.toml");
        let manifest: MemberManifest = match read_toml(&manifest_path) {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        };
        let mut deps = CrateDeps {
            deps: manifest.dependencies.keys().cloned().collect(),
            dev_deps: manifest.dev_dependencies.keys().cloned().collect(),
            build_deps: manifest.build_dependencies.keys().cloned().collect(),
        };
        for target in manifest.target.values() {
            deps.deps.extend(target.dependencies.keys().cloned());
            deps.dev_deps
                .extend(target.dev_dependencies.keys().cloned());
            deps.build_deps
                .extend(target.build_dependencies.keys().cloned());
        }
        crates.insert(manifest.package.name, deps);
    }

    let violations = check(&rules_file.crates, &crates);
    if violations.is_empty() {
        println!(
            "arch: {} 个成员，依赖方向与白名单一致（无环）",
            crates.len()
        );
        return ExitCode::SUCCESS;
    }
    for violation in &violations {
        eprintln!("arch: {}", violation.render());
    }
    eprintln!("arch: {} 处违规", violations.len());
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(entries: &[(&str, &[&str], &[&str], &[&str])]) -> BTreeMap<String, CrateRule> {
        entries
            .iter()
            .map(|(name, may, dev, forbid)| {
                (
                    name.to_string(),
                    CrateRule {
                        may_depend_on: may.iter().map(|item| item.to_string()).collect(),
                        dev_may_depend_on: dev.iter().map(|item| item.to_string()).collect(),
                        forbid: forbid.iter().map(|item| item.to_string()).collect(),
                    },
                )
            })
            .collect()
    }

    fn crate_deps(deps: &[&str], dev: &[&str]) -> CrateDeps {
        CrateDeps {
            deps: deps.iter().map(|item| item.to_string()).collect(),
            dev_deps: dev.iter().map(|item| item.to_string()).collect(),
            build_deps: BTreeSet::new(),
        }
    }

    #[test]
    fn allowed_edges_pass() {
        let rules = rules(&[("a", &[], &[], &[]), ("b", &["a"], &[], &[])]);
        let crates = BTreeMap::from([
            ("a".to_string(), crate_deps(&[], &[])),
            ("b".to_string(), crate_deps(&["a"], &[])),
        ]);
        assert!(check(&rules, &crates).is_empty());
    }

    #[test]
    fn undeclared_internal_dependency_fails() {
        let rules = rules(&[("a", &[], &[], &[]), ("b", &[], &[], &[])]);
        let crates = BTreeMap::from([
            ("a".to_string(), crate_deps(&[], &[])),
            ("b".to_string(), crate_deps(&["a"], &[])),
        ]);
        assert_eq!(
            check(&rules, &crates),
            vec![Violation::Undeclared {
                krate: "b".into(),
                dep: "a".into(),
                dev: false,
            }]
        );
    }

    #[test]
    fn forbidden_third_party_fails() {
        let rules = rules(&[("a", &[], &[], &["reqwest"])]);
        let crates = BTreeMap::from([("a".to_string(), crate_deps(&["reqwest"], &[]))]);
        assert_eq!(
            check(&rules, &crates),
            vec![Violation::Forbidden {
                krate: "a".into(),
                dep: "reqwest".into(),
            }]
        );
    }

    #[test]
    fn dev_dependency_needs_its_own_allowance() {
        let rules = rules(&[("a", &[], &[], &[]), ("b", &[], &[], &[])]);
        let crates = BTreeMap::from([
            ("a".to_string(), crate_deps(&[], &[])),
            ("b".to_string(), crate_deps(&[], &["a"])),
        ]);
        assert_eq!(
            check(&rules, &crates),
            vec![Violation::Undeclared {
                krate: "b".into(),
                dep: "a".into(),
                dev: true,
            }]
        );
    }

    #[test]
    fn cycle_is_reported() {
        let rules = rules(&[("a", &["b"], &[], &[]), ("b", &["a"], &[], &[])]);
        let crates = BTreeMap::from([
            ("a".to_string(), crate_deps(&["b"], &[])),
            ("b".to_string(), crate_deps(&["a"], &[])),
        ]);
        let violations = check(&rules, &crates);
        assert!(
            violations
                .iter()
                .any(|violation| matches!(violation, Violation::Cycle(_))),
            "expected a cycle violation, got {violations:?}"
        );
    }

    #[test]
    fn unregistered_member_and_unknown_entry_are_both_reported() {
        let rules = rules(&[("ghost", &[], &[], &[])]);
        let crates = BTreeMap::from([("a".to_string(), crate_deps(&[], &[]))]);
        let violations = check(&rules, &crates);
        assert!(violations.contains(&Violation::Unregistered("a".into())));
        assert!(violations.contains(&Violation::UnknownCrate("ghost".into())));
    }
}
