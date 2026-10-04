//! 契约编辑（保格式）：`[resources.skills].use` 的增删。
//!
//! 与 `av skill add/remove` 共用一份实现 —— CLI 与桌面应用的技能面板都改
//! 同一批用户文件，编辑语义（注释与格式保留、空表清理、落盘前 schema
//! 校验）必须一致，不能各写一套。
//!
//! [`edit_use_names`] 是唯一带校验的入口：编辑结果先过
//! [`AgentToml::validate`](crate::schema::AgentToml::validate) 再交给调用方
//! 落盘（fail-closed）。文件读写仍由调用方负责——原子写用
//! [`write_text_atomic`](crate::store::write_text_atomic)。

use crate::schema::AgentToml;

/// 保证 `[resources.skills].use` 恰好包含 `add`、不包含 `remove`（保格式），
/// 并做契约 schema 校验。返回 `(新文本, 是否发生变化)`。
///
/// 同名同时出现在两个列表时按「先移除、后添加」处理。字段/表的创建与清理
/// 规则见 [`apply_add`] / [`apply_remove`]。
pub fn edit_use_names(
    text: &str,
    add: &[String],
    remove: &[String],
) -> Result<(String, bool), String> {
    let mut current = text.to_string();
    let mut changed = false;
    for name in remove {
        let (edited, removed) = apply_remove(&current, name)?;
        current = edited;
        changed |= removed;
    }
    if !add.is_empty() {
        let (edited, added) = apply_add(&current, add)?;
        current = edited;
        changed |= added;
    }
    if changed {
        let config: AgentToml =
            toml::from_str(&current).map_err(|e| format!("编辑后的契约无法解析：{e}"))?;
        config.validate()?;
    }
    Ok((current, changed))
}

/// 向 `[resources.skills].use` 追加技能名（去重）。返回 `(新文本, 是否有新增)`。
fn apply_add(text: &str, names: &[String]) -> Result<(String, bool), String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("解析契约失败：{e}"))?;
    let resources = ensure_standard_table(doc.as_table_mut(), "resources")?;
    let skills = ensure_standard_table(resources, "skills")?;
    let array = ensure_use_array(skills)?;
    let mut added = false;
    for name in names {
        if !array
            .iter()
            .any(|value| value.as_str() == Some(name.as_str()))
        {
            array.push(name.as_str());
            added = true;
        }
    }
    Ok((doc.to_string(), added))
}

/// 从 `[resources.skills].use` 移除技能名；数组清空时移除 `use` 键，
/// 连带清理变空的表。返回 `(新文本, 是否找到并移除)`。
fn apply_remove(text: &str, name: &str) -> Result<(String, bool), String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("解析契约失败：{e}"))?;
    let Some(resources) = doc
        .as_table_mut()
        .get_mut("resources")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok((text.to_string(), false));
    };
    let Some(skills) = resources
        .get_mut("skills")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok((text.to_string(), false));
    };
    let Some(item) = skills.get_mut("use") else {
        return Ok((text.to_string(), false));
    };
    let array = item
        .as_array_mut()
        .ok_or("resources.skills.use 不是数组（契约非法）")?;
    let mut found = false;
    let mut index = 0;
    while index < array.len() {
        if array.get(index).and_then(toml_edit::Value::as_str) == Some(name) {
            array.remove(index);
            found = true;
        } else {
            index += 1;
        }
    }
    if !found {
        return Ok((text.to_string(), false));
    }
    if array.is_empty() {
        skills.remove("use");
    }
    // 清理因移除而变空的表（契约保持干净；含其它键的表不动）
    let skills_empty = skills.is_empty();
    if skills_empty {
        resources.remove("skills");
    }
    if resources.is_empty() {
        doc.as_table_mut().remove("resources");
    }
    Ok((doc.to_string(), true))
}

fn ensure_standard_table<'a>(
    table: &'a mut toml_edit::Table,
    key: &str,
) -> Result<&'a mut toml_edit::Table, String> {
    if table.get(key).is_none() {
        table.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    table
        .get_mut(key)
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            format!("契约里的 {key} 不是标准表（内联表/数组形式不支持自动编辑，请手工展开）")
        })
}

fn ensure_use_array(table: &mut toml_edit::Table) -> Result<&mut toml_edit::Array, String> {
    if table.get("use").is_none() {
        table.insert(
            "use",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    }
    table
        .get_mut("use")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(|| "resources.skills.use 不是数组（契约非法）".to_string())
}

#[cfg(test)]
mod tests;
