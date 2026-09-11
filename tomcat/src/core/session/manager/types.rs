//! Context management data structures (TASK-17 / TASK-20 / TASK-21 §5.7).

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use tracing::{info, warn};

use crate::core::compaction::preheat::Preheat;
use crate::core::llm::{
    ChatMessage, ChatMessageContent, ChatMessageRole, EffectiveModelLimits, MessageKind,
};
use crate::infra::error::AppError;
use crate::infra::wire;

// ---------------------------------------------------------------------------
// §5.7 message / turn ids
// ---------------------------------------------------------------------------

/// 复合 TurnId：`start_id + "::" + end_id`（与 [context-management.md §5.7] 一致）。
///
/// MessageId 不得包含子串 `::`；若违反则打日志但仍拼接，避免线上硬崩。
pub fn compound_turn_id(start_id: &str, end_id: &str) -> String {
    if start_id.contains("::") || end_id.contains("::") {
        warn!(
            %start_id,
            %end_id,
            "compound_turn_id: message id should not contain `::` (reserved as turn separator)"
        );
    }
    format!("{start_id}::{end_id}")
}

// ---------------------------------------------------------------------------
// ApiUsage
// ---------------------------------------------------------------------------

/// API token 使用量快照（从 `StreamEvent::Usage` 捕获）。
#[derive(Debug, Clone)]
pub struct ApiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

// ---------------------------------------------------------------------------
// CompactionResult (TASK-20)
// ---------------------------------------------------------------------------

/// 异步预热任务完成后的结果。
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary_text: String,
    pub covered_start_id: String,
    pub covered_end_id: String,
    pub covered_count: usize,
    /// JSONL 中 `Compaction` 行的 `id`；apply 时用于原地将 `isBoundary` 置为 true。
    pub transcript_compaction_entry_id: Option<String>,
    /// L1 预热完成时估算：覆盖区 tokens（旧 transcript 无此字段时为 `None`）。
    pub estimated_covered_tokens_before: Option<usize>,
    pub estimated_summary_tokens: Option<usize>,
    /// L2 apply 时计入 `session_obs.compaction_tokens_freed`（`None` 视为 0）。
    pub estimated_tokens_saved: Option<usize>,
    /// 预热任务耗时（ms）；从 transcript 恢复的 pending 为 0。
    pub preheat_elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// SessionContextObservation / ContextLiveMetrics
// ---------------------------------------------------------------------------

/// 会话级可观测累计：与 [`crate::core::session::store::SessionEntry`] 在 user turn 末同步；**不**含瞬时 ratio/tokens。
#[derive(Debug, Clone, Default)]
pub struct SessionContextObservation {
    /// 成功 apply boundary / L3 trim 等次数（与 `SessionEntry.compaction_count`）。
    pub compaction_count: u32,
    /// 估算释放的 tokens（L0+L2+L3；与 `SessionEntry.compaction_tokens_freed`）。
    pub compaction_tokens_freed: usize,
    /// L0 落盘原始 Unicode 字符数（与 `SessionEntry.tool_result_chars_persisted`；事件字段仍名 bytes）。
    pub tool_result_chars_persisted: usize,
    /// 所有 provider Usage 样本的 prompt token 累计；可用于定位「命中率高但总输入仍过大」。
    pub prompt_tokens_total: u64,
    /// provider 明确报告的 cache read token 累计（缺失的 usage 不伪装成 miss）。
    pub cache_read_tokens_total: u64,
    /// 仅计入 provider 报告了 cache read 指标的 prompt token，用作命中率分母。
    pub cache_observed_prompt_tokens_total: u64,
    /// cache 指标可用的 Usage 样本数；0 表示该 provider 未提供可判读的数据。
    pub cache_observed_request_count: u32,
    /// 连续零 cache-read 的最长长度（未报告 cache 字段不改变这个序列）。
    pub consecutive_cache_miss: u32,
    pub consecutive_cache_miss_max: u32,
    /// 有 Usage 的请求中，runtime tail 相对上一请求发生变化的次数。
    pub tail_changed_count: u32,
    /// tail 变化请求的 prompt token 总量。它是待排查样本规模，不能反推因果损失。
    pub tail_change_miss_tokens: u64,
    /// 不刷盘的请求本地状态：tail 是否变动，供其后的 Usage 样本归因。
    pub(crate) ephemeral_tail_observed: bool,
    pub(crate) last_ephemeral_tail_hash: Option<u64>,
    pub(crate) last_request_tail_changed: bool,
}

impl SessionContextObservation {
    /// Record the exact ephemeral-tail shape used by the next provider request.
    /// Hashing keeps the changing text out of diagnostics and session state.
    pub fn observe_ephemeral_tail(&mut self, tail: Option<&str>) {
        let tail_hash = tail.map(|text| {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            hasher.finish()
        });
        self.last_request_tail_changed =
            self.ephemeral_tail_observed && self.last_ephemeral_tail_hash != tail_hash;
        self.ephemeral_tail_observed = true;
        self.last_ephemeral_tail_hash = tail_hash;
    }

    /// Add one provider Usage sample. An absent `cache_read_tokens` means unavailable, not zero.
    pub fn observe_provider_usage(&mut self, prompt_tokens: u32, cache_read_tokens: Option<u32>) {
        self.prompt_tokens_total += u64::from(prompt_tokens);
        if self.last_request_tail_changed {
            self.tail_changed_count = self.tail_changed_count.saturating_add(1);
            self.tail_change_miss_tokens += u64::from(prompt_tokens);
        }
        let Some(cache_read_tokens) = cache_read_tokens else {
            return;
        };
        self.cache_observed_request_count = self.cache_observed_request_count.saturating_add(1);
        self.cache_observed_prompt_tokens_total += u64::from(prompt_tokens);
        self.cache_read_tokens_total += u64::from(cache_read_tokens);
        if cache_read_tokens == 0 {
            self.consecutive_cache_miss = self.consecutive_cache_miss.saturating_add(1);
            self.consecutive_cache_miss_max = self
                .consecutive_cache_miss_max
                .max(self.consecutive_cache_miss);
        } else {
            self.consecutive_cache_miss = 0;
        }
    }

    pub fn cache_hit_ratio(&self) -> Option<f64> {
        (self.cache_observed_prompt_tokens_total > 0).then(|| {
            self.cache_read_tokens_total as f64 / self.cache_observed_prompt_tokens_total as f64
        })
    }

    pub fn last_request_tail_changed(&self) -> bool {
        self.last_request_tail_changed
    }
}

/// 瞬时上下文指标：仅内存，**不**写入 `sessions.json`。
#[derive(Debug, Clone, Default)]
pub struct ContextLiveMetrics {
    pub input_tokens_used: usize,
    pub context_utilization_ratio: f64,
    pub preheat_in_progress: bool,
    pub preheat_result_pending: bool,
    /// 最近一次 assistant 终局账本；仅内存态，供 CLI / transcript 路径复用。
    pub finish_reason: Option<String>,
    pub error_message: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanEventKind {
    Create,
    Build,
    Update,
}

impl PlanEventKind {
    pub fn from_event_name(name: &str) -> Option<Self> {
        match name {
            wire::WIRE_PLAN_CREATE => Some(Self::Create),
            wire::WIRE_PLAN_BUILD => Some(Self::Build),
            wire::WIRE_PLAN_UPDATE => Some(Self::Update),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEventRef {
    pub kind: PlanEventKind,
    pub plan_id: String,
    pub path: PathBuf,
}

impl PlanEventRef {
    pub fn from_custom_event(extra: &serde_json::Value) -> Option<Self> {
        let obj = extra.as_object()?;
        let event = obj.get("event")?.as_str()?;
        let kind = PlanEventKind::from_event_name(event)?;
        let plan_id = obj.get("plan_id")?.as_str()?.to_string();
        let path = crate::infra::platform::normalize_path(obj.get("path")?.as_str()?).ok()?;
        Some(Self {
            kind,
            plan_id,
            path,
        })
    }
}

// ---------------------------------------------------------------------------
// AgentMode / 控制态恢复
// ---------------------------------------------------------------------------

/// 用户可见的会话模式。
///
/// 计划文件的 `planning` / `pending` / `executing` / `completed` 生命周期不属于会话
/// 模式；它由计划文件 frontmatter 的 `state` 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// `exec` 是 v2 sidecar 里已经落盘的旧值。它在新模型里等价于 Chat：
    /// 执行中的事实由计划文件恢复，而不是一个第三会话模式。
    #[serde(alias = "exec")]
    Chat,
    Plan,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Plan => "plan",
        }
    }
}

/// 一条 transcript 事件对控制态的影响。字段为 `None` 表示该事件不改动这一项。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanModeTransition {
    pub mode: Option<AgentMode>,
    pub plan_id: Option<String>,
    pub path: Option<PathBuf>,
}

impl PlanModeTransition {
    /// 与 [`PlanEventRef::from_custom_event`] 的区别：这里对缺字段是宽容的。
    /// `plan.enter` 发生时计划文件往往还不存在，既没有 plan_id 也没有 path，
    /// 但"进了 PLAN 模式"这件事必须被记住。
    pub fn from_custom_event(extra: &serde_json::Value) -> Option<Self> {
        let obj = extra.as_object()?;
        let event = obj.get("event")?.as_str()?;
        let mode = match event {
            wire::WIRE_PLAN_ENTER => Some(AgentMode::Plan),
            wire::WIRE_PLAN_EXIT => Some(AgentMode::Chat),
            wire::WIRE_PLAN_BUILD | wire::WIRE_PLAN_PENDING | wire::WIRE_PLAN_COMPLETE => {
                Some(AgentMode::Chat)
            }
            wire::WIRE_SESSION_AGENT_MODE_CHANGED => obj
                .get("agentMode")
                .and_then(serde_json::Value::as_str)
                .and_then(|raw| serde_json::from_value(serde_json::Value::String(raw.into())).ok()),
            // create / update 只绑定计划，不改模式。
            wire::WIRE_PLAN_CREATE | wire::WIRE_PLAN_UPDATE => None,
            _ => return None,
        };
        Some(Self {
            mode,
            plan_id: obj
                .get("plan_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            path: obj
                .get("path")
                .and_then(|v| v.as_str())
                .and_then(|raw| crate::infra::platform::normalize_path(raw).ok()),
        })
    }
}

/// 会话恢复时交给 `plan_runtime` 的控制态。由 resume-index sidecar 直接提供，
/// 无需每次遍历 transcript。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumeControlState {
    /// `None` 表示 sidecar 里没有这项信息（旧版本 / 全量扫描路径），走推断兜底。
    pub mode: Option<AgentMode>,
    pub plan_path: Option<PathBuf>,
    pub plan_id: Option<String>,
}

impl ResumeControlState {
    /// 折叠一条事件的影响。
    pub fn apply(&mut self, transition: PlanModeTransition) {
        if let Some(mode) = transition.mode {
            self.mode = Some(mode);
        }
        if transition.plan_id.is_some() {
            self.plan_id = transition.plan_id;
        }
        if transition.path.is_some() {
            self.plan_path = transition.path;
        }
    }
}

// ---------------------------------------------------------------------------
// ContextState
// ---------------------------------------------------------------------------

/// 运行时上下文状态，在 `chat_loop` 外层初始化一次、跨迭代复用。
pub struct ContextState {
    pub messages: Vec<ChatMessage>,
    pub estimate_context_chars: usize,
    pub context_budget_chars: usize,
    pub context_budget_tokens: usize,
    pub last_api_usage: Option<ApiUsage>,
    pub post_usage_appended_chars: usize,
    /// 当前 session 的 transcript 文件路径，供异步预热 spawn 闭包 clone。
    pub transcript_path: PathBuf,
    /// 单次反向扫描识别出的最近一条 `plan.*` transcript 自定义事件。
    pub latest_plan_event: Option<PlanEventRef>,
    /// 会话恢复时的模式与计划绑定，优先由 resume-index sidecar 提供。
    pub resume_control: ResumeControlState,
    /// 异步预热状态机（替代旧 `Option<CompactionSummary>`）。
    pub preheat: Preheat,
    /// 会话累计（刷盘子集）。
    pub session_obs: SessionContextObservation,
    /// 瞬时指标（`AgentLoop` 经方案 1 只写此处，不写独立 `metrics`）。
    pub live: ContextLiveMetrics,
}

fn _assert_send<T: Send>() {}
#[allow(dead_code)]
fn _static_assert_context_state_send() {
    _assert_send::<ContextState>();
}

impl ContextState {
    /// Apply the limits of the model selected for the next request. This never
    /// restores compacted history; it only changes the next compaction
    /// threshold when a session switches models.
    pub fn apply_limits(&mut self, limits: &EffectiveModelLimits) {
        self.context_budget_tokens = limits.input_budget_tokens;
        self.context_budget_chars = limits.input_budget_tokens * 4;
    }

    /// 追加消息后增量更新估算字符数和 post-usage 增量。
    pub fn on_message_appended(&mut self, content_len: usize) {
        self.estimate_context_chars += content_len;
        self.post_usage_appended_chars += content_len;
    }

    /// Record an assistant response that is already included in the provider's
    /// `completion_tokens`. Keep it in the full fallback estimate, but do not
    /// add it to the post-usage delta or the usage-backed estimate would count
    /// the same output twice.
    pub fn on_assistant_message_appended(&mut self, content_len: usize) {
        self.estimate_context_chars += content_len;
    }

    /// 估算当前上下文占用的 token 数。
    /// 有 API usage 时基于真实值 + 增量近似；否则 fallback 字符估算。
    pub fn estimated_token_count(&self) -> usize {
        if let Some(ref usage) = self.last_api_usage {
            let base = (usage.prompt_tokens + usage.completion_tokens) as usize;
            base + self.post_usage_appended_chars / 4
        } else {
            self.estimate_context_chars / 4
        }
    }

    /// 当前上下文利用率（0.0 ~ inf）。
    /// `context_budget_tokens == 0` 时返回 `f64::MAX` 以安全触发 Layer 3。
    pub fn usage_ratio(&self) -> f64 {
        if self.context_budget_tokens == 0 {
            return f64::MAX;
        }
        self.estimated_token_count() as f64 / self.context_budget_tokens as f64
    }

    /// LLM 返回 Usage 事件后刷新真实 token 计数，清零增量。
    pub fn update_api_usage(&mut self, prompt_tokens: u32, completion_tokens: u32) {
        self.last_api_usage = Some(ApiUsage {
            prompt_tokens,
            completion_tokens,
        });
        self.post_usage_appended_chars = 0;
    }

    /// compaction 后使 API usage 失效，后续 fallback 到字符估算。
    pub fn invalidate_api_usage(&mut self) {
        self.last_api_usage = None;
        self.post_usage_appended_chars = 0;
    }

    /// Replace the session-level system-prompt contribution to the fallback
    /// character estimate. API usage is deliberately invalidated because its
    /// prompt-token base was measured against the previous system prefix.
    pub fn replace_system_prompt_chars(&mut self, old_chars: usize, new_chars: usize) {
        self.estimate_context_chars = self
            .estimate_context_chars
            .saturating_sub(old_chars)
            .saturating_add(new_chars);
        self.invalidate_api_usage();
    }

    /// mid-turn 改写 current tail 文本后，同步修正内存估算与 appended delta。
    /// 仅适用于「发生在最后一次 Usage 之后」的本轮局部消息改写。
    pub fn rewrite_local_tail_chars(&mut self, old_chars: usize, new_chars: usize) {
        if new_chars >= old_chars {
            let delta = new_chars - old_chars;
            self.estimate_context_chars += delta;
            self.post_usage_appended_chars += delta;
            return;
        }

        let delta = old_chars - new_chars;
        self.estimate_context_chars = self.estimate_context_chars.saturating_sub(delta);
        self.post_usage_appended_chars = self.post_usage_appended_chars.saturating_sub(delta);
    }

    /// 当前上下文是否超预算（token 维度）。
    pub fn is_over_budget(&self) -> bool {
        self.estimated_token_count() > self.context_budget_tokens
    }

    /// 将已完成的 CompactionResult 应用到 messages 列表：
    /// 找到最后一条 `msg_id == covered_end_id` 的消息，将其及之前所有消息替换为摘要消息。
    /// 无匹配时返回 [`AppError::ApplyBoundaryStale`]。
    pub fn apply_boundary(&mut self, result: CompactionResult) -> Result<(), AppError> {
        let end_idx = self
            .messages
            .iter()
            .rposition(|m| m.msg_id.as_deref() == Some(result.covered_end_id.as_str()))
            .ok_or(AppError::ApplyBoundaryStale {
                covered_end_id: result.covered_end_id.clone(),
            })?;

        let batch_chars: usize = self.messages[..=end_idx]
            .iter()
            .map(estimate_msg_chars)
            .sum();
        let replaced_message_ids = self.messages[..=end_idx]
            .iter()
            .filter_map(|message| message.msg_id.as_deref())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let summary_chars = result.summary_text.len();

        let summary_entry_id = result
            .transcript_compaction_entry_id
            .clone()
            .unwrap_or_else(|| compound_turn_id(&result.covered_start_id, &result.covered_end_id));
        let summary_msg = ChatMessage::compaction_summary(&result.summary_text, summary_entry_id);

        self.messages.splice(..=end_idx, [summary_msg]);
        self.estimate_context_chars =
            self.estimate_context_chars.saturating_sub(batch_chars) + summary_chars;
        self.invalidate_api_usage();
        info!(
            target: "tomcat_chat_diag",
            phase = "history_rewritten",
            operation = "apply_boundary",
            chars_freed = batch_chars.saturating_sub(summary_chars),
            ?replaced_message_ids,
            summary_chars,
        );
        Ok(())
    }

    /// 当前上下文中的 turn 数：user 消息 + compaction 摘要消息之和。
    pub fn turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role == ChatMessageRole::User || m.kind == MessageKind::CompactionSummary)
            .count()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// 与 `ContextState::estimated_token_count` 的纯字符 fallback 一致：`chars / 4`。
#[inline]
pub fn estimated_tokens_from_chars(chars: usize) -> usize {
    chars / 4
}

/// 估算单条 ChatMessage 的「字符等价长度」（用于 `estimate_context_chars` fallback）。
///
/// 多模态 `Parts` 调用 [`ChatMessageContentPart::estimated_chars`](crate::core::llm::types::ChatMessageContentPart::estimated_chars)
/// 折算（IMAGE_CHAR_ESTIMATE = 3600 / FILE_CHAR_ESTIMATE = 8000，常量定义在
/// [`crate::core::llm::types`] 顶部），从而与 `OpenAiProvider::count_tokens` /
/// `OpenAiResponsesProvider::count_tokens` 的分子口径对齐——保证 `ContextState::estimated_token_count`
/// 在首轮 stream 完成、`last_api_usage` 还是 `None` 时不会把多模态请求体积当成 0。
pub fn estimate_msg_chars(msg: &ChatMessage) -> usize {
    let content_len = match &msg.content {
        Some(ChatMessageContent::Text(s)) => s.len(),
        Some(ChatMessageContent::Parts(parts)) => {
            parts.iter().map(|p| p.estimated_chars()).sum::<usize>()
        }
        None => 0,
    };
    let tc_len = msg.tool_calls.as_ref().map_or(0, |tcs| {
        tcs.iter().map(|tc| tc.to_string().len()).sum::<usize>()
    });
    content_len + tc_len
}
