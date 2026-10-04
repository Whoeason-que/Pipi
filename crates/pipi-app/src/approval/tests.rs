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
    assert!(
        gate.resolve("nonexistent", ApprovalDecision::Allow)
            .is_err()
    );

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
