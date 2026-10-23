//! Tauri command 参数的扁平反序列化适配。
//!
//! 前端调用是 `invoke(cmd, args)`，`args` 是**扁平**对象；而 `#[tauri::command]`
//! 默认按参数名从 payload 取键，因此 `args: XxxArgs` 会去找 `payload.args`，
//! 前端从不这样嵌套。这里改为从整个 payload 反序列化 `pipi_core::commands` 里
//! 定义的共享参数结构体（serde camelCase），使薄壳只需声明
//! `args: InvokeArgs<XxxArgs>`，线协议与旧实现完全一致。

use serde::de::{DeserializeOwned, Error as _};
use tauri::ipc::{CommandArg, CommandItem, InvokeBody, InvokeError};

/// 从 invoke payload 顶层反序列化的命令参数。
pub struct InvokeArgs<T>(pub(crate) T);

impl<'de, T, R> CommandArg<'de, R> for InvokeArgs<T>
where
    T: DeserializeOwned,
    R: tauri::Runtime,
{
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        match command.message.payload() {
            InvokeBody::Json(payload) => {
                let json = if payload.is_null() {
                    serde_json::json!({})
                } else {
                    payload.clone()
                };
                serde_json::from_value(json)
                    .map(InvokeArgs)
                    .map_err(|error| {
                        tauri::Error::InvalidArgs(command.name, command.key, error).into()
                    })
            }
            InvokeBody::Raw(_) => Err(tauri::Error::InvalidArgs(
                command.name,
                command.key,
                serde_json::Error::custom("Command arguments must be a JSON object"),
            )
            .into()),
        }
    }
}
