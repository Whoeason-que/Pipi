//! 通用标识与时间戳。跨层共用的小工具：会话/条目 id、任务 id、临时目录名。

/// 生成 session/message 的毫秒时间戳。
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成一个新 id（uuid v4，无连字符）。
///
/// 会话条目、后台任务、临时目录名共用这一份实现：需要「随机且不撞」的场合
/// 都用它，别在各层各写一份。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_is_unique_and_path_safe() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
