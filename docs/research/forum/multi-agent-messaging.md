# 多 Agent 消息 / 订阅语义调研

> 面向 Pipi「Agent 本地论坛」的调研。对象：MetaGPT（共享消息池 / Environment）、AgentScope（消息交换 / pub-sub / 分布式服务）、AutoGen（GroupChat / GroupChatManager 的调度与终止）。
>
> 调研方式：`reference/metagpt`、`reference/agentscope` 为本地只读浅克隆；AutoGen 未克隆，按 `microsoft/autogen` 的 `main` 分支源码在线核对（2026-10）。行号以当前克隆 / 当前 `main` 为准。
>
> 一句话结论：**三者的“消息内容”语义可以搬，进程内共享对象和调用栈调度不能搬。**Pipi 论坛应以“持久化帖子 + 每 Agent 未读游标 + run 时注入摘要”为事实源，把“后台唤醒”降级为可选、限流、可预算的提示信号。

---

## 0. 结论先行

1. **MetaGPT 是“中央环境转发 + 角色私有收件箱 + 双重过滤”**：`send_to` 决定谁收到，`cause_by/watch` 决定谁真的处理；决策仍由各角色自己发起，没有中央“下一个谁说话”的调度器。
2. **AgentScope 同时提供了两套模型**：进程内 `TeamPipeline` / `GoalPipeline` 是显式调用链；应用层 `app/message_bus` 才是真正的异步 / 分布式消息层（持久队列、重放日志、分布式锁、唤醒派发器）。后者最接近 Pipi 需要的论坛底座。
3. **AutoGen 是“共享群聊主题 + 中央 manager”**：所有参与者把消息发布到 group topic，manager 决定下一个发言者；终止靠可组合的 `TerminationCondition` 与 `max_turns`，而不是“没新消息就停”。
4. **三者都没有直接提供论坛所需的“按主题订阅 + 未读 + 因果深度 + 防互刷”**。AutoGen 的 @mention 仅用于辅助选发言者；AgentScope 的 `TeamSay` 是按名字广播 / 点对点；MetaGPT 的 `send_to` 是角色 / 动作地址，不是主题。
5. **给 Pipi**：默认**拉式**（run 启动时注入未读摘要，工具按需读全文）；`@mention` / 回复我 才进入“可唤醒”队列；后台派发器只负责**合并后的唤醒信号**，事实仍从持久化论坛重建。必须加冷却、最大因果深度、每帖轮次上限、每日预算与去重。

---

## 1. 发布 / 订阅模型

### 1.1 MetaGPT：中央环境 + 角色私有收件箱 + 动作订阅

关键事实：

- `Environment` 持有 `roles`、`member_addrs`、`history`，角色可向环境发布消息，其他角色观察。`environment/base_env.py:153-176`。
- `Environment.publish_message()` 遍历所有角色的地址集合，命中 `is_send_to(message, addrs)` 就调用该角色的 `put_message()`；没有收件人时只告警。`environment/base_env.py:175-198`。
- `is_send_to()` 的规则：`send_to` 含 `<all>` → 所有角色；否则角色地址与 `send_to` 有交集 → 命中。`utils/common.py:423-431`；哨兵常量在 `const.py:78-83`（`<all>` / `<none>` / `<self>`）。
- 角色默认地址是“角色类路径 + 角色名”；也可显式 `set_addresses()`。`roles/role.py:210-214`、`roles/role.py:293-301`。
- 消息进入角色私有 `MessageQueue`（`asyncio.Queue`）`rc.msg_buffer`，不是直接进入共享历史。`roles/role.py:98-102`、`roles/role.py:448-452`、`schema.py:713-750`。
- 角色在 `_observe()` 里做第二层过滤：
  `n.cause_by in self.rc.watch or self.name in n.send_to`，并排除已处理消息。`roles/role.py:399-421`。
- `watch` 默认是 `UserRequirement`，各角色按动作类型订阅。例如 ProjectManager 关注 `UserRequirement/PrepareDocuments`，Engineer 关注 `WriteTasks/SummarizeCode/WriteCode/...`。`roles/role.py:170-178`、`roles/product_manager.py:45-47`、`roles/engineer.py:106`。

**语义总结**：

- 传输上是**广播 / 多播**（默认 `<all>`，每条消息复制到所有地址命中的角色缓冲区）。
- 消费上是**按动作类型订阅**（`cause_by`）+ 按名字 / 地址点名（`send_to`）。
- 地址不是 topic，而是“角色名、角色类路径、动作类路径”这类字符串；`cause_by` 更接近“事件类型订阅”。
- 共享的 `Environment.history` 只用于调试，角色推理读取的是自己的 `rc.memory`；所以是**共享传输、私有记忆**。

### 1.2 AgentScope：两层模型（进程内 Pipeline + 应用层 MessageBus）

AgentScope 当前版本（`2.0.11dev`）需要分两层看。

**(a) 进程内 Pipeline**

- `TeamPipeline`：leader 通过 `TeamAssign` 工具把任务派给成员；成员在**自己的上下文**里运行，结果作为 tool result 回到 leader。成员之间不直接对话：“Members do not talk to each other; a member's result always returns to the leader.” `pipeline/_team_pipeline.py:110-126`。
- `GoalPipeline`：executor / verifier 循环，直到通过或 `max_iters`（默认 10）耗尽。`pipeline/_goal_pipeline.py:55-87`、`pipeline/_goal_pipeline.py:314`。
- 这是显式调用链，不是 pub/sub；没有 topic、订阅、未读。

**(b) 应用层消息总线（`app/message_bus`）**

- 总线明确区分“实时传输”与“持久记录”：storage 负责持久记录，bus 只提供 push-capable 的实时协调。`app/message_bus/_base.py:1-39`。
- 三种消费语义：
  - **Mode A drain queue**：单消费者、读即删、ack-on-read；
  - **Mode C replay log**：多消费者、各自游标、按 `max_len` / TTL 截断；
  - **Mode D transient broadcast**：fire-and-forget，只给当前订阅者，离线期间丢失。
  `app/message_bus/_base.py:15-45`。
- 总线**故意不提供 counted broadcast**：要 fan-out 给 N 个成员，就由生产者在写入时逐个 push 到 N 个收件箱；去重也是生产者责任。`app/message_bus/_base.py:37-42`。
- session inbox 是每个 session 独立的 drain queue：`MessageBusKeys.inbox(sid)`；另有 inbox 锁与“当前 run 是否在消费”的 consumer 标记。`app/message_bus/_keys.py:201-245`。
- 唤醒队列是共享的 durable trigger queue（`wakeups`），另有 transient signal 通道。`app/message_bus/_keys.py:248-265`。

**TeamSay 的寻址**：

- `TeamSay(content, to)`：`to=None` 表示广播给除自己外的所有团队成员；`to="name"` 表示按**显示名**点对点。`app/_tool/_team_say.py:25-31`、`app/_tool/_team_say.py:121-145`。
- 团队目录由 storage 中的 team roster 解析；leader 用普通名字，受邀成员用 `name@agentId[:8]` 避免重名。`app/_tool/_team_say.py:180-220`。
- 投递不是“写一条群消息”，而是对每个 recipient 调用 `deliver_to_inbox()`：把 `HintBlock` 推进该 session 的 inbox，并在没有 run 消费时入队唤醒。`app/_tool/_team_say.py:269-290`、`app/_bus_ops.py:163-204`。
- `Msg` 本身**没有 recipient 字段**；地址只存在于投递动作 / session 维度。`app/message/_base.py:70-115`。

**语义总结**：

- 应用层是**按 session 收件箱的写入时 fan-out**，不是共享群聊日志；`TeamSay` 的“广播”是生产者逐个写 N 份。
- 订阅关系不在消息里，而在“谁被写入了 inbox”这个动作里；topic 等价物是 session / team。
- 进程内 Pipeline 是同步调用链，不能直接当作异步论坛协议。

### 1.3 AutoGen：共享 group topic + 中央 manager

- `BaseGroupChat` 为每个 team 生成唯一 topic：`group_topic_<teamId>`、每个 participant 的 `<name>_<teamId>`、manager 的 `<manager>_<teamId>`、输出 topic。`teams/_group_chat/_base_group_chat.py:105-127`。
- 参与者同时订阅自己的 participant topic 与 group topic；manager 订阅自己的 topic 与 group topic。`_base_group_chat.py:191-243`。
- 文档明确：group chat 中参与者通过**把消息发布给所有其他参与者**来共享上下文。`_base_group_chat.py:40-55`。
- manager 收到 participant response 后，调用 `select_speaker()` 决定下一轮发言者，并把请求路由过去。`teams/_group_chat/_base_group_chat_manager.py:172-193`、`:306-319`。
- 除群聊外，`HandoffMessage` 用 `target` 字段表达点对点交接。`messages.py:421-430`。

**语义总结**：

- 传输是**共享 group topic 的广播**；所有参与者看到同一份消息流。
- “下一个谁说话”由中央 manager 决定，不是订阅者自取。
- 没有用户可配置的“主题订阅 / 未读”概念；`@mention` 只出现在 Selector 的选人逻辑里（见 §3）。

### 1.4 三者对比

| 维度 | MetaGPT | AgentScope（应用层） | AutoGen |
| --- | --- | --- | --- |
| 传输 | 中央 Environment 转发到角色私有队列 | 每 session inbox；写入时逐收件人 fan-out | 共享 group topic 广播 |
| 订阅依据 | `send_to` 地址 + `cause_by/watch` 动作类型 | 谁被写入 inbox（team roster / 名字） | 参与者订阅 group topic |
| 地址 | 角色名 / 类路径 / 动作类路径 | session id + 成员显示名 | topic type + participant name；Handoff target |
| 下一个发言者 | 无中央调度；有 news 的角色并发运行 | 应用层由 wake trigger 启动 run；进程内由 leader 工具调用 | 中央 manager（round-robin / selector） |
| 共享历史 | 有 `env.history`，但仅调试；推理用私有 memory | 无共享群聊历史；各 session 私有 context | 有，所有参与者共享 group 消息流 |

---

## 2. 消息结构

### 2.1 MetaGPT `Message`

`metagpt/schema.py:232-242`：

| 字段 | 含义 |
| --- | --- |
| `id` | 消息 id，缺省 uuid |
| `content` | 自然语言正文 |
| `instruct_content` | 可选结构化输出（Pydantic / ActionNode） |
| `role` | `system` / `user` / `assistant` |
| `cause_by` | 由哪个 Action 产生（事件类型 / 订阅键） |
| `sent_from` | 发送者（类路径或名字） |
| `send_to` | 收件人集合，默认 `{"<all>"}` |
| `metadata` | 任意附加元数据 |

特点：**有发送者、收件人、产生原因，没有时间戳、没有 reply/parent id、没有 topic id**。

### 2.2 AgentScope `Msg` 与 `HintBlock`

`Msg`（`message/_base.py:70-115`）：

| 字段 | 含义 |
| --- | --- |
| `name` | 发送者名字 |
| `content` | `ContentBlock` 列表（text / thinking / data / tool call / tool result） |
| `role` | `user` / `assistant` / `system` |
| `id` | 消息 id |
| `metadata` | 任意元数据 |
| `created_at` | 创建时间 |
| `usage` | token 用量 |
| `finished_at` / `finished_reason` | 回复完成时间 / 终止原因（completed / interrupted / error / exceed_max_iters） |
| `structured_output` / `error` | 结构化输出 / 结构化错误 |

特点：**有消息 id、时间、终止原因、用量；没有 recipient、topic、reply/parent id**。团队通知不是 `Msg`，而是 `HintBlock`（`message/_block.py:101`），由 `InboxMiddleware` 注入上下文（`app/middleware/_inbox_middleware.py:24-130`）。

### 2.3 AutoGen `BaseChatMessage`

`messages.py:68-93`：

| 字段 | 含义 |
| --- | --- |
| `source` | 发送者名字 |
| `content` | 文本内容 |
| `type` | 消息类型（`TextMessage` 等） |
| `models_usage` | token 用量 |
| `metadata` | 附加元数据 |

`HandoffMessage` 额外有 `target`（目标 agent 名字）。`messages.py:421-430`。
特点：**有发送者、类型、用量；没有接收者列表、topic、时间戳（由外层事件携带）、reply/parent id**。

### 2.4 论坛需要补齐的字段

三者都不足以直接表达论坛。Pipi 至少需要：

- `topicId`：帖子所属主题；
- `postId` / `parentId`：回复因果链（三者都没有）；
- `author`（Agent 名 / `human`）；
- `createdAt`；
- `mentions[]`：被 @ 的 Agent；
- `kind`：`human` / `agent` / `system` / `moderator`；
- `originRunId` / `sessionId`：哪个 run 产生，便于幂等与审计；
- `causalityDepth`（可由 parent 链推导，但建议落盘）；
- 订阅 / 游标不进消息，放在独立状态里。

---

## 3. 调度 / 触发：谁决定“接下来谁说话”

### 3.1 MetaGPT：无中央调度的“有消息就并发反应”

- `Environment.run(k)` 每一轮遍历所有角色，跳过 idle 的，对非 idle 角色并发 `asyncio.gather(role.run())`。`environment/base_env.py:197-208`。
- 角色 `run()` 先 `_observe()`；**没有新消息就直接返回**（“no news. waiting.”）。`roles/role.py:530-555`。
- 角色 `is_idle` = 没有 news、没有 todo、收件箱为空。`roles/role.py:557-559`；环境 `is_idle` = 所有角色 idle。`environment/base_env.py:229-235`。
- 因此“下一个谁说话”是**涌现的**：谁收到感兴趣的消息，谁就在这一轮运行；没有 manager。
- 角色内部还有 `max_react_loop`，是“一次反应里做几个动作”的上限，不是跨角色轮次。`roles/role.py:108-113`、`roles/role.py:454-471`。

### 3.2 AgentScope：应用层“显式唤醒”，进程内“显式调用”

- **无新消息就停**：`InboxMiddleware` 在每次 reasoning step 前 drain inbox；没有条目就不注入。`app/middleware/_inbox_middleware.py:81-130`。
- **应用层触发**：`WakeupDispatcher` 是每进程一个 asyncio task，订阅 signal 通道并 drain durable trigger queue；它是**唯一**把 `ChatService.run` spawn 进 `ChatRunRegistry` 的地方。`app/_manager/_wakeup_dispatcher.py:1-35`、`:65-136`。
- trigger 有三种 `kind`：`wake`（空闲 session，无输入，只 drain inbox）、`resume`（HITL 恢复，携带事件）、`message`（真实用户消息，携带 `Msg`）。`app/message_bus/_keys.py:27-52`、`app/_bus_ops.py:72-138`。
- 生产侧只在“没有 run 注册为 inbox consumer”时为 `wake` 入队；若派发时 session 仍持有锁，实现不会并发启动第二个 run，而是带 backoff 重新入队。`resume` / `message` 同样重排队、不丢弃。`_wakeup_dispatcher.py:12-23`、`:279-353`、`app/_bus_ops.py:140-204`。
- run 结束前会检查 inbox：如果还有 pending，就**留在同一个 run 里再来一轮**；没有才释放 consumer 标记并结束。`app/_service/_chat.py:1432-1450`。
- 所有 run 都在 session 分布式锁下执行，避免同一 session 两个 run 并写。`app/_service/_chat.py:813-816`。

### 3.3 AutoGen：manager 显式选人

- `RoundRobinGroupChat`：manager 按参与者顺序轮转。`teams/_group_chat/_round_robin_group_chat.py:72-83`。
- `SelectorGroupChat`：manager 让模型根据 `selector_prompt` 从候选参与者中选一个；默认 `max_selector_attempts=3`，`allow_repeated_speaker=False`。`teams/_group_chat/_selector_group_chat.py:152-217`、`:597-620`。
- 选人结果必须恰好命中一个参与者名字；命中多个或零个都会反馈重试。`_selector_group_chat.py:252-307`。
- 这里没有“无新消息就停”：group chat 是主动推进的，直到终止条件或 `max_turns` 触发。`_base_group_chat_manager.py:195-228`。

### 3.4 对比

| 问题 | MetaGPT | AgentScope | AutoGen |
| --- | --- | --- | --- |
| 谁发起下一轮 | 有 news 的角色自己 | wake trigger / 已有 run 的 inbox 检查 | 中央 manager |
| 触发方式 | 事件驱动（消息到达私有队列） | 事件驱动 + 显式 spawn | 轮次驱动 / 模型选人 |
| 无新消息 | 角色 return，环境 idle | run 检查 inbox，空则结束 | 不会自动停，需终止条件 |
| 单次反应上限 | `max_react_loop` | `ReActConfig.max_iters`（默认 50） | 由调用方 / 终止条件控制 |

---

## 4. 循环与失控防护

### 4.1 MetaGPT

- **总轮数**：`Team.run(n_round=3)`，每轮 `env.run()`；`env.is_idle` 时提前 break。`team.py:123-138`。
- **预算**：`CostManager.max_budget`（默认 10.0），`Team._check_balance()` 超预算抛 `NoMoneyException`。`team.py:98-100`、`utils/cost_manager.py:29-59`。
- **单角色动作上限**：`max_react_loop`，注释明确“防止无限反应”。`roles/role.py:261-281`。
- **去重**：`Memory.add()` 对已在 `storage` 中的消息直接 return；`_observe` 还排除 `old_messages`。`memory/memory.py:24-33`、`roles/role.py:408-411`。
- 不足：没有因果深度、没有每对 Agent 的轮次上限、没有冷却；默认 `<all>` 广播 + 多角色 watch 仍可能在 `n_round` 内互相触发；预算只在轮边界检查。

### 4.2 AgentScope

- **单次回复上限**：`ReActConfig.max_iters` 默认 50；到达上限后先强制一次“总结、不要调工具”的 finalization，再以 `EXCEED_MAX_ITERS` 结束。`agent/_config.py:362-384`、`agent/_agent.py:3656-3674`。
- **会话串行**：session 分布式锁保证同一 session 只有一个 run；inbox consumer 标记 + 锁解决“最后 drain 之后又来了消息”的丢失唤醒问题。`app/_bus_ops.py:140-204`、`_chat.py:1432-1450`。
- **唤醒不丢**：`resume` / `message` 在 session 忙时重新入队；`wake` 只在没有 run 消费 inbox 时由生产侧入队，派发时若仍忙也重排队。`_wakeup_dispatcher.py:12-23`、`:279-353`。
- **团队报告约束**：`TeamMemberLoopMiddleware` 要求成员以成功 `TeamSay` 给 leader 报告结尾；未报告最多 nudge 3 次，之后以 `ERROR` 结束，避免在 session 锁下无限循环。`app/middleware/_team_member_middleware.py:21-48`、`:148-205`。
- **提示词层面的反轮询**：TeamSay 的 leader 描述明确“不要反复调用检查进度，成员完成会自动通知”；worker 描述要求完成后必须报告。`app/_tool/_team_say.py:50-77`。
- 不足：`WakeupDispatcher` 对忙 session 的 trigger **无限重排队**（带 backoff），没有“每主题最大轮次”或“每日唤醒预算”；inbox 消费是 at-most-once，去重仍由生产者负责；`TeamSay(to=None)` 会 fan-out 给所有成员，可能放大讨论。

### 4.3 AutoGen

- 可组合终止条件：`MaxMessageTermination(max_messages)`、`TextMentionTermination("TERMINATE")`、`TokenUsageTermination`、`StopMessageTermination`、`FunctionalTermination`。`conditions/_terminations.py:57-155`、`:229-272`。
- `max_turns` 在 manager 中检查，达到就发 `StopMessage` 并结束；同时重置终止条件与 turn 计数。`_base_group_chat_manager.py:195-228`。
- Selector 默认禁止连续同一发言者（`allow_repeated_speaker=False`），选人失败最多 3 次后回退到上一个 / 第一个参与者。`_selector_group_chat.py:597-620`、`:288-307`。
- 不足：终止条件是“调用方配置”的，不是论坛式的“讨论自然收敛”；没有冷却、因果深度、每对 Agent 的互刷限制；共享群聊上下文会持续增长。

### 4.4 防护手段对照

| 手段 | MetaGPT | AgentScope | AutoGen |
| --- | --- | --- | --- |
| 最大总轮数 | `n_round` | 每 reply `max_iters`；run 可续 | `max_turns` |
| 预算 | `max_budget` | 无内置（可外层计费） | `TokenUsageTermination` |
| 显式终止信号 | 无（idle） | 无（inbox 空） | `TextMention` / `StopMessage` / functional |
| 去重 | `Memory.add` + old_messages | at-most-once queue；生产方负责 | 无内建 |
| 防连续同一人 | 无 | 无 | `allow_repeated_speaker` |
| 冷却 / 因果深度 / 每帖上限 | 无 | 无 | 无 |

---

## 5. 记忆 / 上下文：共享历史 vs 私有历史

### 5.1 MetaGPT：共享传输、私有推理记忆

- `Environment.history` 记录所有发布消息，注释明确 “For debug”。`environment/base_env.py:151`、`:196`。
- 角色推理用的是 `rc.memory`；`_observe` 只把“感兴趣”的 news 写入 memory（除非 `observe_all_msg_from_buffer=True`）。`roles/role.py:408-421`。
- `Memory` 按 `cause_by` 建索引，支持 `get_by_action(s)`、`find_news`。`memory/memory.py:16-105`。
- 因此“之前说过什么”对角色来说是**被过滤后的私有历史**，不是完整群聊。

### 5.2 AgentScope：每 session 私有上下文 + 独立的持久消息 / 重放日志

- `AgentState.context` 是当前 session 的私有上下文；`reply_context` 记录 `reply_id` / `cur_iter`。`state/_state.py:183-221`。
- 持久层 `StorageBase` 有 `upsert_message` / `get_message` / `list_messages`（游标分页）。`app/storage/_base.py:738-818`。
- session 事件另走 replay log：`publish_session_event()` 先 `log_append(max_len=1000)` 再 `publish` 实时通知。`app/_bus_ops.py:30-53`、`app/message_bus/_keys.py:124-128`。
- 团队消息以 `HintBlock` 注入到当前 session 的上下文中，而不是合并成共享群聊记录。`app/middleware/_inbox_middleware.py:24-130`。
- `TeamPipeline` 中 leader **只看到成员的最终 summary**，看不到成员中间步骤。`pipeline/_team_pipeline.py:113-121`。
- 结论：**私有会话上下文是事实，跨 Agent 的“共享”靠显式消息 / 重放日志，不靠共享 memory 对象。**

### 5.3 AutoGen：共享群聊上下文

- 参与者把消息发布到 group topic，所有参与者都收到；manager 维护 `_message_thread`。`_base_group_chat.py:40-55`、`_base_group_chat_manager.py:298-304`。
- Selector 还会维护一个 `model_context` 用于选人。`_selector_group_chat.py:232-246`。
- 所以 AutoGen 的“之前说过什么”是**全组共享**的；上下文增长和 token 压力也由全组承担。

### 5.4 对论坛的启示

- **共享的是帖子存储，不是 Agent 的上下文**。每个 Agent 应该只把“与我相关的未读 + 我参与过的线程摘要”注入自己的 run。
- 需要**每 Agent × 每主题的游标**；不能用全局“最后一条已读”。
- 需要区分“投递（delivered）”与“已读（read）”：投递成功不等于 Agent 已经理解；游标只应在成功 run / 成功读取后推进。
- 论坛帖子应保留完整因果链；Agent 上下文只放“摘要 + 最近 N 条 + 可展开的引用”。

---

## 6. 异步与持久化：哪些能搬，哪些搬不过来

### 6.1 可搬运性矩阵

| 语义 | 可否直接搬到 Pipi 持久化异步论坛 | 依据 / 原因 |
| --- | --- | --- |
| 消息信封（发送者 / 内容 / 类型 / 时间 / id） | ✅ | 三者都有，补齐 topic/parent/mentions 即可 |
| 按主题 / 角色 / 名字寻址 | ✅ | MetaGPT `send_to`、AgentScope `TeamSay(to=name)`、AutoGen topic |
| 动作 / 事件类型订阅 | ✅ | MetaGPT `cause_by + watch` 可直接映射为 `kind` / 事件标签 |
| 每收件人一份 inbox | ✅ | AgentScope 明确“fan-out at write time，逐收件人 push” |
| 持久队列 + at-most-once 消费 | ✅ | AgentScope Mode A；Pipi 可用文件追加 + 原子重命名 / 游标 |
| 重放日志 + 每读者游标 | ✅ | AgentScope Mode C；适合论坛帖子流 |
| 唤醒信号与事实分离 | ✅ | AgentScope Mode D 只做 wake-up；事实在 storage / queue |
| 单 session 串行 / 运行锁 | ✅ | AgentScope session lock；Pipi 已有 run 守卫 / `reserve_run` |
| 空闲唤醒派发器 | ⚠️ 有条件 | 需要一个常驻宿主（Pipi 的 Tokio runtime 可以），但必须单派发器、可合并、可限流、可预算 |
| 进程内共享 `Memory` / `Environment.history` | ❌ | 生命周期等于进程；Agent 不常驻，重启即丢 |
| `asyncio.Queue` 收件箱 | ❌ | 只在进程内；Pipi 退出后不可恢复 |
| `Environment.run()` 的 `gather` 调度 | ❌ | 假设所有角色同时活着；Pipi 的 Agent 是按 run 唤醒的 |
| AutoGen group chat manager 调用栈 | ❌ | manager / participants 是运行时对象；只能搬“选人 + 终止条件”的逻辑 |
| 共享群聊上下文 | ❌ | 需要从持久帖子重建；不能依赖进程内广播 |
| 瞬时 broadcast 当作事实源 | ❌ | 离线期间丢失；只能当“有新内容”的提示 |

### 6.2 关键区分

**能搬的是“语义”，不是“实现”**：

- MetaGPT 的 `send_to` + `cause_by` 可以搬成论坛的 `mentions` + `subscription.kind`。
- AgentScope 的 `inbox + wakeup + consumer marker + session lock` 可以搬成 Pipi 的“持久 inbox + 单派发器 + run 守卫”。
- AutoGen 的 `TerminationCondition` 可以搬成论坛的“话题终止策略”（最大帖数、最大 token、显式 `[settled]`、人工关闭）。

**搬不过来的核心原因**：

- MetaGPT / AutoGen 的调度都依赖“所有 Agent 对象同时在内存里”；Pipi 的 Agent 是按 run 临时装配的。
- AgentScope 的进程内 `TeamPipeline` 也是同一个进程里的调用链；真正可搬的是它**应用层**的 bus + storage 分层。
- 所有框架的“共享历史”都不是持久化格式；Pipi 必须把论坛历史落成文件，并在每次 run 时重建上下文。

### 6.3 AgentScope 给出的最重要启发

它把三件事拆开了：

1. **事实**：storage / 持久消息；
2. **传输**：durable queue / replay log / transient signal；
3. **执行**：session lock + run + inbox consumer。

论坛也应该这样拆：帖子文件是事实，inbox 是待办，wake 只是提示；不要用 wake 信号本身承载“有新帖”这一事实。

---

## 7. 给 Pipi 的结论

### 7.1 推荐通知模型：拉式为主，推式为辅

**默认：拉式（pull）**

- 论坛写入时只做两件事：追加帖子；给订阅者 / 被 @ 者写一条持久 inbox 记录（或只推进“未读集合”）。
- Agent 每次 run 启动时：
  1. 读自己的 `forumCursor`；
  2. 取未读帖子的**摘要**（标题、作者、时间、父帖、@我、前 N 条回复）；
  3. 以受 token 上限约束的 system / user 提示注入；
  4. 需要全文时由模型调用 `forum_read(postId)`。
- run 结束后，只有“成功处理”才推进游标；失败 / 中断不推进。
- 优点：完全符合“Agent 不是常驻进程”；不依赖后台派发；重启后仍能从文件恢复；成本可控。

**可选：推式（push）只做“唤醒”，不做“传输”**

- 适用于 `@mention`、回复我、我显式 watch 的主题。
- 由一个**单例** forum notifier 消费持久 wake 队列；合并同一 Agent 的多个唤醒；若该 Agent 已有 run 则跳过 / 延后；唤醒时只传“去读论坛”的信号，不传正文。
- 唤醒必须落在一个**专用 forum session / run**，不要打断用户当前聊天会话；同一 Agent 同一时刻最多一个论坛 run。
- 默认关闭；按 Agent / 按主题 opt-in，并受每日唤醒次数与预算限制。

**推荐优先级**：

```
@mention / 回复我的帖子  → 持久 inbox + 可唤醒（若策略允许）
我 watch 的主题新帖      → 只记未读，下次 run 注入
普通订阅主题新帖         → 只记未读，低优先级摘要
广播 / 全站公告          → 只进摘要，不唤醒
```

**为什么不是纯推式**：Pipi 进程可能关闭；纯推式会丢事件或需要常驻进程。**为什么不是纯拉式**：@mention 需要及时性；没有唤醒，讨论会停在“等下一次用户打开”。混合模型把“及时性”限定在高信号事件上，避免每个新帖都唤醒所有 Agent。

### 7.2 mention / 订阅语义

**寻址**

- `@name` 使用**精确 Agent 名**（Pipi 的 Agent 名已由 `validate_agent_name` 约束，可作为稳定地址）。
- 只匹配正文中的 mention token；忽略代码块 / 引用块中的 `@`；忽略自己 @ 自己。
- 一条帖子最多 N 个 mention（建议 3），超出截断并记录系统告警。
- mention 只产生“通知 / 未读”，不自动授予“必须回复”的义务；是否回复由 Agent 在 run 中决定。

**订阅层级**

| 层级 | 语义 | 默认 |
| --- | --- | --- |
| `off` | 不接收该主题未读 | 新建主题默认 |
| `mentions` | 只接收 @我 / 回复我 | 被 @ 后自动升级？建议不自动，提示用户 / Agent 显式开启 |
| `watch` | 接收该主题所有新帖摘要 | 主题参与者可显式开启 |
| `all` | 接收全站新帖摘要 | 不建议，仅管理员 / 摘要用 |

**游标**

- 每 `(agent, topic)` 一条 `lastReadPostId`（或单调序号），外加 `lastNotifiedPostId`。
- 不要只存全局游标；否则一个 Agent 在 A 主题读完后会漏掉 B 主题。
- `delivered`、`read`、`replied` 三态分开：投递成功不等于已读；已读不等于已回复。
- 失败 / 中断的 run 不推进 `lastReadPostId`；重复投递用 `postId` 去重。

**因果与回复**

- 回复必须带 `parentId`；`rootId` 可直接取父帖的 `rootId`，根帖 `rootId = postId`。
- 通知“回复我”只看 `parentId` 的直接父作者，不因整条祖先链反复通知。
- 默认不自动订阅整条线程；需要显式 watch。

### 7.3 必备护栏清单

以下默认值是可实施的起点，不是产品最终值。

**循环 / 刷屏**

- [ ] 每个主题最大帖数 / 时间窗（如 24h 内 20 帖）。
- [ ] 每个 Agent 每主题每时间窗最大发帖数（如 8 帖 / 24h）。
- [ ] 连续 Agent 发帖上限（如 2 条）；超过后必须等待人类或新的 @mention。
- [ ] 每 Agent × 主题冷却（如 5–10 分钟）；全局冷却（如 1 分钟）。
- [ ] 每条新帖最多触发 1 次自动回复；回复不再自动触发同一 Agent。
- [ ] 广播 / `to=all` 的 fan-out 上限（如 10 个收件人），超出改为“摘要 + 不唤醒”。

**因果深度**

- [ ] `maxCausalDepth`（如 4；根帖深度 0）。超过深度的回复只入库，不再唤醒 / 不再自动回复。
- [ ] 对“Agent 发的帖”与“人类发的帖”使用不同策略：Agent 帖默认不自动唤醒其他 Agent，除非被 @ 或主题处于“自动讨论”模式。
- [ ] 记录并展示 `causalityDepth`、`rootId`、`parentId`，便于审计和 UI 折叠。

**预算**

- [ ] 每次 run 的未读注入 token 上限（如 4k tokens）。
- [ ] 每 Agent / 每主题 / 每日 token 或费用上限。
- [ ] 每天自动唤醒 run 次数上限（如 10 次 / Agent）。
- [ ] 论坛回复的模型调用计入现有 run 预算，不另开无上限调用。

**去重 / 幂等**

- [ ] 帖子内容规范化后计算 hash；同 `(topicId, parentId, author, hash)` 拒绝重复。
- [ ] 自动回复使用幂等键 `(sourcePostId, agentName, runId)`，重试不产生重复帖。
- [ ] mention 去重；同一帖子对同一 Agent 只产生一条 inbox 记录。
- [ ] 线程级“已处理”集合，避免同一个 Agent 对同一帖重复回复。

**终止 / 人工控制**

- [ ] 主题有 `status: open / settled / locked`；`settled` 后不再唤醒，只保留只读。
- [ ] 支持 `[settled]` / `[no further action]` 这类显式结束标记，但必须配合“人工可关闭”；不能只依赖模型自停。
- [ ] 主题级 kill switch / mute；Agent 级 mute。
- [ ] 失败 run 不推进游标；失败重试有次数与退避。

**可观测性**

- [ ] 记录每次唤醒的原因（mention / reply / watch / manual）、被合并的条数、是否跳过（session busy / budget）。
- [ ] 记录自动回复链长度、被护栏拦截的次数。
- [ ] UI 显示“未读 / 已读 / 已回复 / 已静音”，并允许人类一键停止某条讨论。

### 7.4 最小数据模型（文件优先）

建议把论坛作为**全局持久对象**，不要塞进某个 Agent 的会话目录：

```
~/.pipi/forum/
  topics/<topicId>/topic.toml        # 标题、作者、状态、createdAt、订阅默认值
  topics/<topicId>/posts/<seq>-<postId>.md
  agents/<agentName>/subscriptions.toml
  agents/<agentName>/cursors.toml    # topicId -> lastReadPostId / lastNotifiedPostId
  agents/<agentName>/inbox.jsonl     # 待处理通知（可重建）
```

帖子头部（YAML / TOML frontmatter 或独立的 metadata）：

```text
id, topicId, parentId, rootId, author, kind,
createdAt, mentions[], causalityDepth, originRunId, contentHash
```

正文用 Markdown；引用父帖用 id，不复制全文。inbox 可重建，游标与帖子是事实源。

### 7.5 建议的 v1 范围

- **v1 只做拉式**：`forum_list` / `forum_read` / `forum_post` / `forum_reply` 四个工具；run 启动注入未读摘要；不做后台唤醒。这样先把数据模型、游标、去重、因果深度跑稳。
- **v1.5 加 mention 通知**：只写 inbox，不启动 run；下次 run 注入高优先级摘要。
- **v2 再加可选的 mention 唤醒**：单派发器 + 合并 + 冷却 + 每日预算 + 专用 forum session；默认关闭。
- **不要**在 v1 做“所有订阅帖都自动唤醒所有 Agent”；这是最容易失控的形态。

### 7.6 反模式

- ❌ 把论坛做成“所有 Agent 都在线的群聊”，每条消息广播并期待所有人回复。
- ❌ 用 session 当日志、把同一个 session 同时当作用户聊天和论坛讨论；应使用独立 forum session / run。
- ❌ 只存“已读游标”而不存帖子因果链；无法判断谁回复了谁。
- ❌ 用瞬时 pub/sub 承载“有新帖”的事实；进程重启 / 离线即丢。
- ❌ 让 Agent 的帖子自动触发所有订阅者，且没有连续发帖上限与冷却。
- ❌ 把整条线程塞进上下文；应注入摘要 + 按需读取。
- ❌ 认为“模型说 TERMINATE”就足够；必须有硬性帖数 / 预算 / 人工关闭。

---

## 8. 证据索引

### MetaGPT

- `reference/metagpt/metagpt/environment/base_env.py:151-242`：Environment 字段、`publish_message`、`run`、`is_idle`、地址设置。
- `reference/metagpt/metagpt/roles/role.py:98-113`：`RoleContext.msg_buffer` / `memory` / `watch` / `max_react_loop`。
- `reference/metagpt/metagpt/roles/role.py:293-301`：`set_addresses`。
- `reference/metagpt/metagpt/roles/role.py:399-452`：`_observe` 过滤、`publish_message`、`put_message`。
- `reference/metagpt/metagpt/roles/role.py:530-559`：`run` 无 news 返回、`is_idle`。
- `reference/metagpt/metagpt/schema.py:232-242`：`Message` 字段；`:713-750`：`MessageQueue`。
- `reference/metagpt/metagpt/memory/memory.py:16-105`：去重、按 `cause_by` 索引、`find_news`。
- `reference/metagpt/metagpt/team.py:98-138`：预算检查、`n_round`、`is_idle` 退出。
- `reference/metagpt/metagpt/utils/cost_manager.py:29-59`：`max_budget` / `total_cost`。
- `reference/metagpt/metagpt/utils/common.py:423-431`：`is_send_to`。
- `reference/metagpt/metagpt/const.py:78-83`：路由哨兵。

### AgentScope

- `reference/agentscope/src/agentscope/message/_base.py:70-115`：`Msg` 字段。
- `reference/agentscope/src/agentscope/message/_block.py:101`：`HintBlock`。
- `reference/agentscope/src/agentscope/app/message_bus/_base.py:1-45`：三种消费模式、无 counted broadcast、去重责任。
- `reference/agentscope/src/agentscope/app/message_bus/_keys.py:201-265`：inbox / wakeup 键与 TTL。
- `reference/agentscope/src/agentscope/app/_bus_ops.py:30-53`：replay log + live publish。
- `reference/agentscope/src/agentscope/app/_bus_ops.py:72-204`：run trigger、`deliver_to_inbox`、inbox consumer 协议。
- `reference/agentscope/src/agentscope/app/_manager/_wakeup_dispatcher.py:1-35`、`:211-353`：单派发器、kind、忙时重排队。
- `reference/agentscope/src/agentscope/app/_service/_chat.py:813-816`、`:1432-1450`：session 锁、run 末尾 inbox 复查。
- `reference/agentscope/src/agentscope/app/middleware/_inbox_middleware.py:24-130`：inbox drain / HintBlock 注入。
- `reference/agentscope/src/agentscope/app/_tool/_team_say.py:25-31`、`:121-145`、`:269-290`：按名字 / 广播、逐收件人 inbox。
- `reference/agentscope/src/agentscope/app/middleware/_team_member_middleware.py:21-48`、`:148-205`：必须报告、nudge 上限。
- `reference/agentscope/src/agentscope/agent/_config.py:362-384`：`ReActConfig.max_iters`。
- `reference/agentscope/src/agentscope/agent/_agent.py:3656-3674`：达到 max iters 的终止。
- `reference/agentscope/src/agentscope/pipeline/_team_pipeline.py:110-126`：leader 派活、成员不互谈。
- `reference/agentscope/src/agentscope/pipeline/_goal_pipeline.py:55-87`、`:314`：executor/verifier 循环与 `max_iters`。
- `reference/agentscope/src/agentscope/app/storage/_base.py:738-818`：消息持久化 / 游标分页。
- `reference/agentscope/src/agentscope/app/storage/_model/_team.py`、`_session.py`：team roster、session 归属。
- `reference/agentscope/src/agentscope/app/message_bus/_redis_message_bus.py:44-78`：Redis Stream / Pub/Sub 映射与“离线不补发”的 wake 语义。
- `reference/agentscope/src/agentscope/app/message_bus/_in_memory_message_bus.py:1-20`：单进程实现，不可用于多进程生产。

### AutoGen（未克隆，按 `microsoft/autogen` `main` 核对）

路径前缀：`python/packages/autogen-agentchat/src/autogen_agentchat/`

- `teams/_group_chat/_base_group_chat.py:40-55`：group chat 共享上下文。
- `teams/_group_chat/_base_group_chat.py:66-127`：`max_turns` / termination、topic 命名。
- `teams/_group_chat/_base_group_chat.py:191-243`：participant / manager 的订阅。
- `teams/_group_chat/_base_group_chat_manager.py:172-228`：选人、终止条件、`max_turns`。
- `teams/_group_chat/_base_group_chat_manager.py:298-319`：`update_message_thread` / `select_speaker`。
- `teams/_group_chat/_round_robin_group_chat.py:72-83`：轮转选人。
- `teams/_group_chat/_selector_group_chat.py:152-217`：selector 选人。
- `teams/_group_chat/_selector_group_chat.py:252-341`：mention 匹配、重试、回退。
- `teams/_group_chat/_selector_group_chat.py:597-620`：默认 `max_selector_attempts=3`、`allow_repeated_speaker=False`。
- `conditions/_terminations.py:57-155`、`:229-272`：最大消息 / 文本终止 / token 用量终止。
- `messages.py:68-93`：`BaseChatMessage` 字段。
- `messages.py:421-430`：`HandoffMessage.target`。

---

## 9. 最终建议（一页版）

- **事实源**：论坛帖子文件（topic + post + parent/root + mentions + kind + createdAt）。
- **通知**：每 Agent 每主题游标 + 持久 inbox；`@mention` / 回复我 才可唤醒。
- **默认拉**：run 启动注入“未读摘要 + 高优先级 mention”；全文工具按需读取。
- **可选推**：单例派发器、合并唤醒、只在 Agent 空闲且策略允许时启动**专用 forum session**；不打断用户聊天。
- **护栏**：最大因果深度 4、每主题 24h 20 帖、每 Agent 每主题 8 帖、连续 Agent 2 帖、冷却 5–10 分钟、每帖 fan-out ≤3 mention、未读注入 ≤4k tokens、每日唤醒 ≤10 次、幂等键去重、失败不推进游标、`settled` / 人工 kill switch。
- **不要搬**：MetaGPT 的共享 `Memory` / `asyncio.Queue`、AutoGen 的共享群聊上下文、任何“所有 Agent 常驻并互相广播”的调度模型。
