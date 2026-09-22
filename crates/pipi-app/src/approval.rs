//! bash 命令的交互审批。
//!
//! 职责切分：核心只维护「请求登记 → 决议回传」的状态机与 fail-closed 语义
//! （超时 / 中止 / 通道断开一律拒绝），传输由宿主完成 —— `InteractiveApprover`
//! 通过 [`crate::runtime::EventEmitter`] 把 [`RuntimeEvent::ApprovalRequest`]
//! 发给宿主，宿主把用户决定经 `resolve_approval` 送回。安全基线不变：
//! 黑名单 / 危险命令 / 沙箱约束在权限层已被硬拒，这里只处理白名单未命中的
//! 非危险命令。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::agents;
use crate::permissions::CommandApprover;
use crate::runtime::{EventEmitter, RuntimeEvent};
use crate::types::AbortSignal;

/// 用户对一次审批请求的决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    /// 仅本次允许。
    Allow,
    /// 本次允许，并把未命中段落写入 Agent 的 bash 白名单（agent.json）。
    Always,
    Deny,
}

impl std::str::FromStr for ApprovalDecision {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "allow" => Ok(Self::Allow),
            "always" => Ok(Self::Always),
            "deny" => Ok(Self::Deny),
            other => Err(format!(
                "未知审批决定「{other}」（可选：allow / always / deny）"
            )),
        }
    }
}

/// 发给宿主的审批请求。`missing` 是白名单未命中的段落，「总是允许」时按段写入。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestEnvelope {
    pub request_id: String,
    pub agent_name: String,
    pub session_id: String,
    pub run_id: usize,
    pub command: String,
    pub missing: Vec<String>,
}

/// 审批请求登记表：request_id → 决议回传端。
#[derive(Default)]
pub struct ApprovalGate {
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
    next_id: AtomicU64,
}

impl ApprovalGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个请求；返回回传端与请求 ID。
    fn register(&self) -> (String, oneshot::Receiver<ApprovalDecision>) {
        let (tx, rx) = oneshot::channel();
        let id = format!(
            "approval-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default(),
            self.next_id.fetch_add(1, Ordering::AcqRel),
        );
        self.pending
            .lock()
            .expect("审批登记表锁中毒")
            .insert(id.clone(), tx);
        (id, rx)
    }

    /// 回传决议。请求不存在或已被终结（超时 / 中止）时返回 Err —— 宿主应据此
    /// 把前端对话框置为失效，而不是让用户以为批准成功了。
    pub fn resolve(&self, request_id: &str, decision: ApprovalDecision) -> Result<(), String> {
        let sender = self
            .pending
            .lock()
            .expect("审批登记表锁中毒")
            .remove(request_id)
            .ok_or_else(|| "审批请求不存在或已过期".to_string())?;
        let _ = sender.send(decision);
        Ok(())
    }

    /// 移除未决请求（超时 / 中止路径），避免登记表泄漏。
    fn cancel(&self, request_id: &str) {
        self.pending
            .lock()
            .expect("审批登记表锁中毒")
            .remove(request_id);
    }
}

/// 审批等待上限。用户在界面上有 10 分钟响应窗口；超时按拒绝处理（fail-closed）。
pub const APPROVAL_TIMEOUT_SECS: u64 = 600;

/// 面向交互宿主的 [`CommandApprover`] 实现：每次运行构造一个，携带本次运行的
/// 身份与中止信号。`session_allowed` 记住本运行内已批准的完整命令（跨运行的
/// 持久化走 agent.json 白名单，由「总是允许」触发）。
pub struct InteractiveApprover {
    gate: Arc<ApprovalGate>,
    emit: EventEmitter,
    agent_name: String,
    session_id: String,
    run_id: usize,
    abort: AbortSignal,
    timeout: Duration,
    session_allowed: Mutex<HashSet<String>>,
}

impl InteractiveApprover {
    pub fn new(
        gate: Arc<ApprovalGate>,
        emit: EventEmitter,
        agent_name: String,
        session_id: String,
        run_id: usize,
        abort: AbortSignal,
    ) -> Self {
        Self {
            gate,
            emit,
            agent_name,
            session_id,
            run_id,
            abort,
            timeout: Duration::from_secs(APPROVAL_TIMEOUT_SECS),
            session_allowed: Mutex::new(HashSet::new()),
        }
    }

    /// 测试钩子：覆盖默认审批等待时长。
    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait::async_trait]
impl CommandApprover for InteractiveApprover {
    async fn approve(&self, command: &str) -> Result<(), String> {
        if self
            .session_allowed
            .lock()
            .expect("审批会话集合锁中毒")
            .contains(command)
        {
            return Ok(());
        }

        let (request_id, rx) = self.gate.register();
        (self.emit)(RuntimeEvent::ApprovalRequest(ApprovalRequestEnvelope {
            request_id: request_id.clone(),
            agent_name: self.agent_name.clone(),
            session_id: self.session_id.clone(),
            run_id: self.run_id,
            command: command.to_string(),
            missing: Vec::new(),
        }));

        let result = tokio::select! {
            decision = rx => match decision {
                Ok(ApprovalDecision::Allow) => Ok(()),
                Ok(ApprovalDecision::Always) => self.persist_allowlist(command),
                Ok(ApprovalDecision::Deny) => Err("用户拒绝了该命令".into()),
                // 发送端已被移除（登记表被清理）——按失败处理，不放行
                Err(_) => Err("审批通道已断开".into()),
            },
            _ = self.abort.wait_aborted() => Err("已中止".into()),
            _ = tokio::time::sleep(self.timeout) => Err(format!(
                "审批超时（{APPROVAL_TIMEOUT_SECS} 秒未响应），命令未执行"
            )),
        };

        // 无论结果如何都清登记：resolve 路径已移除，这里兜底超时 / 中止路径。
        self.gate.cancel(&request_id);
        if result.is_ok() {
            self.session_allowed
                .lock()
                .expect("审批会话集合锁中毒")
                .insert(command.to_string());
        }
        result
    }
}

impl InteractiveApprover {
    /// 「总是允许」：把未命中段落写回 agent.json 白名单。仅 Allowlist 模式有
    /// 持久化意义（Denylist 模式写条目会变成禁令，绝不写）；写入失败不影响
    /// 本次放行 —— 命令已经过用户明示同意。
    fn persist_allowlist(&self, command: &str) -> Result<(), String> {
        let Ok(def) = agents::load_agent(&self.agent_name) else {
            // Agent 定义读取失败时仍按本次批准放行，只是无从持久化
            return Ok(());
        };
        if def.permissions.bash.mode != crate::permissions::BashMode::Allowlist {
            return Ok(());
        }
        let missing =
            crate::permissions::split_segments(command)
                .map(|segments| {
                    segments
                        .into_iter()
                        .filter(|segment| {
                            !def.permissions.bash.commands.iter().any(|entry| {
                                crate::permissions::matches_entry(entry, &segment.text)
                            })
                        })
                        .map(|segment| segment.text)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
        if missing.is_empty() {
            return Ok(());
        }
        agents::add_bash_allowlist_entries(&self.agent_name, &missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured_emitter(sink: Arc<Mutex<Vec<ApprovalRequestEnvelope>>>) -> EventEmitter {
        Arc::new(move |event| {
            if let RuntimeEvent::ApprovalRequest(envelope) = event {
                sink.lock().unwrap().push(envelope);
            }
        })
    }

    fn make_approver(
        gate: Arc<ApprovalGate>,
        sink: Arc<Mutex<Vec<ApprovalRequestEnvelope>>>,
        timeout: Duration,
    ) -> Arc<InteractiveApprover> {
        Arc::new(
            InteractiveApprover::new(
                gate,
                captured_emitter(sink),
                "tester".into(),
                "session-1".into(),
                7,
                AbortSignal::new(),
            )
            .with_timeout(timeout),
        )
    }

    async fn wait_for_requests(sink: &Mutex<Vec<ApprovalRequestEnvelope>>, count: usize) {
        for _ in 0..200 {
            if sink.lock().unwrap().len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("审批请求未在预期时间内送达");
    }

    #[tokio::test]
    async fn allow_resolves_and_remembered_for_session() {
        let gate = Arc::new(ApprovalGate::new());
        let sink = Arc::new(Mutex::new(Vec::new()));
        let approver = make_approver(gate.clone(), sink.clone(), Duration::from_secs(5));

        let pending = {
            let approver = approver.clone();
            tokio::spawn(async move { approver.approve("npm install").await })
        };
        wait_for_requests(&sink, 1).await;
        {
            let requests = sink.lock().unwrap();
            assert_eq!(requests[0].command, "npm install");
            assert_eq!(requests[0].run_id, 7);
            assert_eq!(requests[0].agent_name, "tester");
            let request_id = requests[0].request_id.clone();
            gate.resolve(&request_id, ApprovalDecision::Allow).unwrap();
        }
        pending.await.unwrap().unwrap();

        // 同命令第二次不再询问（会话内记忆）
        let before = sink.lock().unwrap().len();
        approver.approve("npm install").await.unwrap();
        assert_eq!(sink.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn deny_fails_closed_and_expired_request_cannot_resolve() {
        let gate = Arc::new(ApprovalGate::new());
        let sink = Arc::new(Mutex::new(Vec::new()));
        let approver = make_approver(gate.clone(), sink.clone(), Duration::from_secs(5));

        let pending = {
            let approver = approver.clone();
            tokio::spawn(async move { approver.approve("curl example.com").await })
        };
        wait_for_requests(&sink, 1).await;
        let request_id = sink.lock().unwrap()[0].request_id.clone();
        gate.resolve(&request_id, ApprovalDecision::Deny).unwrap();
        let error = pending.await.unwrap().unwrap_err();
        assert!(error.contains("拒绝"));

        // 已决请求不可再次决议；未知 ID 同样拒绝
        assert!(gate.resolve(&request_id, ApprovalDecision::Allow).is_err());
        assert!(gate
            .resolve("nonexistent", ApprovalDecision::Allow)
            .is_err());

        // 拒绝无会话记忆：同命令会再次询问
        let pending = {
            let approver = approver.clone();
            tokio::spawn(async move { approver.approve("curl example.com").await })
        };
        wait_for_requests(&sink, 2).await;
        let second_id = sink.lock().unwrap()[1].request_id.clone();
        gate.resolve(&second_id, ApprovalDecision::Deny).unwrap();
        assert!(pending.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn timeout_fails_closed() {
        let gate = Arc::new(ApprovalGate::new());
        let sink = Arc::new(Mutex::new(Vec::new()));
        let approver = make_approver(gate.clone(), sink.clone(), Duration::from_millis(30));

        let error = approver
            .approve("wget http://example.com")
            .await
            .unwrap_err();
        assert!(error.contains("超时"));
        // 登记表已清理：迟到决议报「不存在」
        assert!(gate.resolve("anything", ApprovalDecision::Allow).is_err());
    }

    #[tokio::test]
    async fn abort_fails_closed() {
        let gate = Arc::new(ApprovalGate::new());
        let sink = Arc::new(Mutex::new(Vec::new()));
        let abort = AbortSignal::new();
        let approver = Arc::new(
            InteractiveApprover::new(
                gate,
                captured_emitter(sink),
                "tester".into(),
                "session-1".into(),
                1,
                abort.clone(),
            )
            .with_timeout(Duration::from_secs(30)),
        );

        let pending = tokio::spawn(async move { approver.approve("make test").await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        abort.abort();
        let error = pending.await.unwrap().unwrap_err();
        assert!(error.contains("中止"));
    }
}
