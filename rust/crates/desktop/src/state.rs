use runtime::{ConversationRuntime, PermissionPolicy, Session};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::anchors;
use crate::api_client::DesktopApiClient;
use crate::config::{normalize_model, DesktopConfig};
use crate::event_sink::Sink;
use crate::mcp::{self, DesktopMcp};
use crate::tool_executor::DesktopToolExecutor;

pub type DesktopRuntime = ConversationRuntime<DesktopApiClient, DesktopToolExecutor>;

pub struct DesktopState {
    pub runtime: DesktopRuntime,
    /// Resolved (normalized) model id used for this session.
    #[allow(dead_code)]
    pub model: String,
    /// Whether OPC CEO mode is active for this session.
    #[allow(dead_code)]
    pub opc_mode: bool,
}

impl DesktopState {
    /// Build a fresh `DesktopState` from a config snapshot. The config is the
    /// single source of truth for model, opc_mode, thinking_mode, mcp_servers,
    /// and permission_mode — no secondary disk read happens inside this function.
    pub fn build(
        config: &DesktopConfig,
        cancel_flag: Arc<AtomicBool>,
        sink: Sink,
        session_id: String,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let model = normalize_model(&config.model);
        let opc_mode = config.opc_mode;
        let thinking_mode = config.thinking_mode;

        // Resolve the per-session JSONL path and load if present.
        let session_path = session_jsonl_path(&session_id);
        let session = if let Ok(loaded) = Session::load_from_path(&session_path) {
            eprintln!(
                "[state] resumed session '{session_id}' with {} message(s)",
                loaded.messages.len()
            );
            loaded
        } else {
            if let Some(parent) = session_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            Session::new().with_persistence_path(session_path.clone())
        };
        let provider_client =
            api::ProviderClient::from_model(&model).map_err(|e| e.to_string())?;

        let mut tool_specs: Vec<api::ToolDefinition> = tools::mvp_tool_specs()
            .into_iter()
            .map(|spec| api::ToolDefinition {
                name: spec.name.to_string(),
                description: Some(spec.description.to_string()),
                input_schema: spec.input_schema.clone(),
            })
            .collect();

        // Decision Anchors — desktop-only tool. Pinning an important fact
        // here keeps it surfaced in the system prompt for the rest of the
        // session, so the model doesn't forget it as context fills.
        tool_specs.push(api::ToolDefinition {
            name: "pin_decision".to_string(),
            description: Some(
                "Pin a key decision so it stays salient for the rest of this session. \
                 Use this when the user (or you) settles on something that future turns \
                 must respect: technical choices, naming, constraints, preferences, \
                 forbidden patterns. The pinned title + rationale will be re-injected \
                 into the system prompt of every subsequent turn — even after context \
                 compaction. Keep title under 60 chars; rationale under 200 chars."
                    .to_string(),
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short noun phrase, what was decided (e.g. \"Use Postgres\")."
                    },
                    "rationale": {
                        "type": "string",
                        "description": "One sentence on why this decision was made (the constraint or context)."
                    }
                },
                "required": ["title", "rationale"]
            }),
        });

        // Initialize user-configured MCP servers (best-effort). On success
        // we extend `tool_specs` with the discovered tools and route their
        // calls through the executor.
        let desktop_mcp: Option<DesktopMcp> = match mcp::init(&config.mcp_servers) {
            Ok(opt) => {
                if let Some(ref m) = opt {
                    eprintln!("[state] {}", m.status);
                    tool_specs.extend(m.tool_specs.clone());
                }
                opt
            }
            Err(e) => {
                eprintln!("[state] MCP init failed: {e}");
                None
            }
        };

        let api_client = DesktopApiClient::new(
            provider_client,
            model.clone(),
            true,
            tool_specs,
            thinking_mode,
            cancel_flag,
            sink,
        )?;
        let tool_executor = DesktopToolExecutor::new(desktop_mcp, session_id.clone());
        // Desktop app is local + user-owned; default to full access so
        // bash / WebSearch / etc. just work. Users who want a brake can
        // dial it down in Settings (config.permission_mode).
        let policy_mode = crate::config::parse_permission_mode(&config.permission_mode);
        let policy = PermissionPolicy::new(policy_mode);

        let cwd = std::env::current_dir().unwrap_or_default();
        let date = simple_date();
        let mut system_prompt =
            runtime::load_system_prompt(cwd.clone(), date, std::env::consts::OS, "unknown")
                .unwrap_or_default();

        if opc_mode {
            system_prompt.push(OPC_CEO_SYSTEM_PROMPT.to_string());
        }

        // Inject long-term memory from .claw/memory/ (dreaming consolidation).
        let memory_snapshot = runtime::memory::MemoryStore::open(&cwd).snapshot_for_prompt();
        if !memory_snapshot.is_empty() {
            system_prompt.push(memory_snapshot);
        }

        // Inject summary of enabled skills so the agent knows what's
        // available without paying the cost of loading every SKILL.md.
        let skills_section = crate::skills::enabled_skills_prompt_section();
        if !skills_section.is_empty() {
            system_prompt.push(skills_section);
        }

        // Inject pinned decision anchors so long sessions don't drift
        // away from earlier choices. Caps at the 12 most recent anchors —
        // any more becomes noise in the system prompt.
        let anchors_section = anchors::snapshot_for_prompt(&session_id, 12);
        if !anchors_section.is_empty() {
            system_prompt.push(anchors_section);
        }

        // Soft cap on the CEO's run_turn loop. The runtime defaults to
        // `usize::MAX` (effectively unlimited), but in OPC mode it's
        // possible for the model to get stuck in a tool-use loop (e.g.
        // calling read_file forever). 200 iterations is generous for
        // legitimate multi-step plans (~5-10 sub-agents × 4 follow-up
        // checks each) while still tripping when something is wrong.
        let runtime = DesktopRuntime::new(session, api_client, tool_executor, policy, system_prompt)
            .with_max_iterations(ceo_max_iterations());

        Ok(Self { runtime, model, opc_mode })
    }
}

/// CEO `run_turn` loop ceiling. Override with `CLAWD_CEO_MAX_ITERATIONS`.
fn ceo_max_iterations() -> usize {
    std::env::var("CLAWD_CEO_MAX_ITERATIONS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(200)
}

/// Root directory under which all per-session JSONL files live.
pub fn sessions_dir() -> std::path::PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("opc-desktop").join("sessions")
}

/// Plain-text file recording which session id is currently active. Read on
/// startup so the user lands back in their last session; written by
/// `set_current_session_id` whenever the user switches via the sidebar.
pub fn current_session_id_path() -> std::path::PathBuf {
    sessions_dir().join("current_id")
}

/// Per-session JSONL file. Multiple sessions live side by side, indexed by
/// id (a Unix-secs timestamp string generated at creation time).
pub fn session_jsonl_path(id: &str) -> std::path::PathBuf {
    sessions_dir().join(format!("{id}.jsonl"))
}

/// Generate a fresh session id. Format: `s-{unix_secs}`. We use a stable
/// prefix so the file naming convention is obvious.
pub fn new_session_id() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("s-{secs}")
}

/// Read which session is currently active. If there is no record (first
/// launch), generate a new id, persist it, and return that.
pub fn read_or_init_current_session_id() -> String {
    let path = current_session_id_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    let id = new_session_id();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &id);
    id
}

/// Set the active session id (called when the user switches sessions).
pub fn set_current_session_id(id: &str) -> std::io::Result<()> {
    let path = current_session_id_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, id)
}

/// Return today's date as `YYYY-MM-DD` (UTC). Uses Howard Hinnant's
/// "civil from days" algorithm — correct for the full Gregorian range,
/// no external crate required.
fn simple_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64; // days since 1970-01-01

    // https://howardhinnant.github.io/date_algorithms.html  (civil_from_days)
    let z = days + 719_468;
    let era: i64 = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;                               // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);        // [0, 365]
    let mp = (5 * doy + 2) / 153;                             // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1;                    // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 };           // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

const OPC_CEO_SYSTEM_PROMPT: &str = "\
## OPC CEO Agent 模式\n\
\n\
你是该工作区的 CEO（首席执行官 Agent）。你是一个一人公司（OPC）的核心协调者。\n\
\n\
### 你的职责\n\
1. 接收用户的高层需求，分析并拆解为具体的可执行子任务\n\
2. 根据任务性质将子任务通过 Agent 工具委派给专业 sub-agent\n\
3. 监督 sub-agent 的执行进度（使用 read_file 读取 manifest_file 检查 status 字段）\n\
4. 收集所有 sub-agent 的输出（使用 read_file 读取 output_file），聚合成完整结果\n\
5. 向用户汇报最终结果，质量把关后再输出，不直接转发 sub-agent 原文\n\
\n\
### 可用的专业 sub-agent 角色（通过 Agent 工具的 subagent_type 参数指定）\n\
- `opc-product`     — 产品设计、需求分析、用户研究、PRD 撰写\n\
- `opc-engineering` — 代码实现、技术方案、测试、调试\n\
- `opc-finance`     — 财务分析、成本核算、商业建模、风险评估\n\
- `opc-marketing`   — 内容营销、文案撰写、SEO、社媒分发、转化优化\n\
- `opc-sales`       — 销售外联、邮件序列、提案撰写、异议处理\n\
- `opc-ops`         — 项目管理、流程设计、资源协调\n\
- `opc-legal`       — 合规审查、合同条款、政策分析\n\
\n\
### 路由规则（强制）\n\
当用户请求明显属于某个专业领域时，**你必须先调用 Agent 工具委派给对应 sub-agent**，不要自己回答：\n\
- 邮件序列 / 销售文案 / cold email / 客户外联 / 提案 → `opc-sales`\n\
- 落地页文案 / SEO / 社媒内容 / 营销策略 → `opc-marketing`\n\
- 财务建模 / 成本分析 / 估值 / 风险评估 → `opc-finance`\n\
- PRD / 用户调研 / 产品规划 → `opc-product`\n\
- 代码 / 技术架构 / 测试 / debug → `opc-engineering`\n\
- 合同审查 / 合规 / 隐私政策 → `opc-legal`\n\
- 项目排期 / 流程梳理 / 任务管理 → `opc-ops`\n\
\n\
只有当请求无法归入任何专业领域（如纯闲聊、追问澄清）时，CEO 才直接回答。\n\
\n\
### 工作流程（同步语义）\n\
**Agent 工具是同步阻塞的**：你调用 Agent 后，sub-agent 在后台独立 context 里跑完整任务，你会**直接拿到它的最终输出文本**作为 tool_result。不需要轮询，不需要读 manifest，不需要等待。\n\
\n\
具体流程：\n\
1. 分析用户需求，拆成 1~5 个独立子任务\n\
2. 调 Agent 工具委派每个子任务（sub-agent 在独立 context 跑，不污染你的对话）\n\
3. 每个 Agent 工具调用返回结构：\n\
   ```json\n\
   { \"agent_id\": \"...\", \"role\": \"opc-product\", \"status\": \"completed\",\n\
     \"output\": \"<sub-agent 完整 markdown 报告>\" }\n\
   ```\n\
   失败时 `status: \"failed\"`，多一个 `error` 字段。\n\
4. 拿到所有 sub-agent 的 output 后，**综合产出最终回复给用户**（不要照搬，要消化、对比、整合）\n\
\n\
### ⚠️ 关键原则\n\
- ❌ **不要**用 Agent 工具去\"检查 manifest\"或\"读取已派 agent 的结果\" — 那会创建一个新的 sub-agent\n\
- ❌ **不要** read_file 任何 `.clawd-agents/` 或 `agent-*.json/.md` 文件 — 内容已直接在 tool_result 里\n\
- ✅ Agent 工具的 tool_result 已包含完整 output 文本，**直接用它就行**\n\
- ✅ 一次回复里可以发起多个 Agent 工具调用（并发执行）\n\
\n\
### 🚀 执行优先原则（最重要）\n\
**你是决策者，不是顾问。** 用户来找你是要你把事情做完，不是给他出选择题。\n\
\n\
**立即执行的情况（不要问，直接做）：**\n\
- 需求清晰度 ≥ 70%：直接拆解、委派 sub-agent、汇报结果\n\
- 任务范围内的技术/策略决策（选哪个技术栈、文案风格、分析框架）：自己定，结果里说明理由\n\
- 后续细化工作：做完主任务后，自动继续做你认为有价值的延伸（除非用户叫停）\n\
\n\
**唯一允许暂停询问的情况：**\n\
- 需求根本无法开始（连做什么都不清楚）\n\
- 需要花钱的决策（广告预算、付费 API、订阅）\n\
- 不可逆的生产操作（正式发布、提交审核、发送给真实用户）\n\
- 存在根本性战略分歧（两个方向都合理，用户偏好直接影响整个路径）\n\
\n\
**🚫 严格禁止以下结尾方式：**\n\
- \"需要我进一步细化吗？\"\n\
- \"你想了解哪个部分？\"\n\
- \"要我继续做 X 吗？\"\n\
- \"如果你想要 Y，告诉我一声\"\n\
- 任何把球踢回给用户的问句\n\
\n\
**✅ 正确做法：** 做完一个阶段后，**直接开始下一阶段**，或者直接给出完整的可用交付物（代码/文档/方案），让用户拿到就能用。\
\n\
\n\
### 完整示例\n\
```\n\
用户：帮我设计一款 SaaS 产品并做市场分析\n\
\n\
[CEO 调用 — 一次发起 2 个并发 Agent]\n\
Agent({subagent_type: \"opc-product\", description: \"产品定位与功能设计\", prompt: \"...\"})\n\
Agent({subagent_type: \"opc-marketing\", description: \"目标市场和竞品分析\", prompt: \"...\"})\n\
\n\
[等 sub-agents 跑完，CEO 直接拿到完整 output]\n\
tool_result_1 = { role: \"opc-product\", output: \"# 产品方案...（详细报告）\" }\n\
tool_result_2 = { role: \"opc-marketing\", output: \"# 市场分析...（详细报告）\" }\n\
\n\
[CEO 综合汇报给用户]\n\
\"基于产品和市场两个角度的分析，建议如下：\n\
 - 产品核心功能 X、Y、Z（来自产品分析）\n\
 - 目标用户为 A 群体（来自市场分析）\n\
 - ...\"\n\
```\n\
\n\
### 原则\n\
- 优先并行委派独立任务（一次回复发多个 Agent 调用，比串行快几倍）\n\
- 委派时在 description 写明任务目标，prompt 给足上下文\n\
- 综合产出时要有自己的判断，不只是搬运 sub-agent 原文\n\
- 用中文与用户沟通，除非用户明确要求其他语言\
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpcAgentInfo {
    pub id: String,
    pub subagent_type: String,
    pub status: String,
    pub description: String,
    /// Unix timestamp (seconds) — used by the UI to group agents into
    /// "turns" (manifests created within ~60s of each other) and to render
    /// relative timestamps ("3 分钟前").
    pub created_at_secs: u64,
}

/// Inline image payload forwarded with a user message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImagePayload {
    /// Base64-encoded image bytes (no data-URL prefix).
    pub data: String,
    /// MIME type: `"image/png"`, `"image/jpeg"`, `"image/gif"`, `"image/webp"`.
    pub media_type: String,
}

pub enum WorkerMsg {
    SendMessage {
        text: String,
        /// Optional inline images (vision). Empty vec = text-only turn.
        images: Vec<ImagePayload>,
        responder: std::sync::mpsc::SyncSender<Result<TurnResult, String>>,
    },
    /// Wipe the *current* session: delete its jsonl + create a fresh one
    /// with a brand-new id, and make it the active one.
    ClearSession {
        responder: std::sync::mpsc::SyncSender<Result<(), String>>,
    },
    /// Switch the worker to a different (existing or new) session id. The
    /// id becomes the new "current" id and DesktopState rebuilds against
    /// the matching jsonl file (loading history if it exists).
    SwitchSession {
        new_id: String,
        responder: std::sync::mpsc::SyncSender<Result<(), String>>,
    },
    Reinitialize {
        config: DesktopConfig,
        responder: std::sync::mpsc::SyncSender<Result<(), String>>,
    },
    /// Run a one-shot compaction pass on the current session: summarize
    /// the older half of messages and replace them with a synthetic
    /// summary exchange. Returns `None` if no safe cut-point exists or
    /// the session is too short.
    CompactSession {
        responder:
            std::sync::mpsc::SyncSender<Result<Option<crate::compaction::CompactionReport>, String>>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnResult {
    pub text: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

pub struct AppState {
    pub tx: std::sync::mpsc::SyncSender<WorkerMsg>,
    /// Cancellation flag shared with the worker's `DesktopApiClient`.
    /// The `cancel_turn` command flips this to `true`; the streaming loop
    /// checks it between events and bails out cleanly.
    pub cancel_flag: Arc<AtomicBool>,
    /// Per-task cancellation flags for active long-running tasks.
    ///
    /// Phase 3 update: long tasks now live in the `opc-daemon` process,
    /// so the desktop no longer owns their cancel flags. Kept here as
    /// `dead_code` so any in-process fallback code path can still
    /// register a flag if we ever re-enable foreground execution.
    #[allow(dead_code)]
    pub long_task_cancels:
        Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
}
