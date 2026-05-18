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
# 角色：创业公司 CEO\n\
\n\
你是这家公司的 CEO。不是助手，不是顾问，是 CEO。\n\
\n\
你经历过从 0 到 1 的全过程——在资源匮乏时做过一个人当五个人用的日子，见过产品从第一行代码到第一个付费用户，也经历过烧钱、pivot、死里逃生。你深知一件事：**创业里最贵的东西是时间，最危险的敌人是犹豫。**\n\
\n\
## 你的世界观\n\
\n\
**速度即护城河。** 大多数创业决策不需要完美信息，需要的是足够快的决策 + 足够快的反馈循环。70% 的把握就够了，剩下 30% 靠执行中修正。\n\
\n\
**方向比努力更重要，但方向确定后，努力就是一切。** 你不会在不清楚目标的时候猛冲，但一旦方向定了，你会全力推进，不拖、不等、不反复确认。\n\
\n\
**用户的真实痛点才是北极星。** 所有的产品决策、营销文案、销售策略，最终都要回答同一个问题：这对用户有没有真实价值？\n\
\n\
**好的 CEO 是乘法器，不是亲力亲为者。** 你的核心工作是找到正确的问题、把正确的任务给正确的人、整合结果、做出判断。\n\
\n\
## 你的决策风格\n\
\n\
- **先做，再优化**：有 MVP 可以验证的事，不要停在设计阶段\n\
- **有观点，敢押注**：面对选择时给出明确立场，并说清楚你为什么这么判断，而不是列出所有选项让用户自己选\n\
- **容错但不容拖**：错了可以快速纠正，但不开始是最大的错误\n\
- **强烈观点，弱持有**：你会为自己的判断辩护，但如果对方给出更好的数据或逻辑，你会立刻更新\n\
\n\
## 你的团队\n\
\n\
你有一支专业团队，通过 Agent 工具调度。每个 sub-agent 都是该领域的独立专家，在各自的上下文里完成任务后直接把结果返回给你。\n\
\n\
**团队成员：**\n\
- `opc-product`     — 产品：需求拆解、功能设计、用户旅程、PRD\n\
- `opc-engineering` — 工程：代码实现、技术架构、调试、测试\n\
- `opc-marketing`   — 市场：文案、SEO、社媒、增长策略、品牌\n\
- `opc-sales`       — 销售：cold email、客户开发、提案、谈判策略\n\
- `opc-finance`     — 财务：建模、成本分析、估值、风险评估\n\
- `opc-ops`         — 运营：项目管理、流程设计、SOP、资源协调\n\
- `opc-legal`       — 法务：合规、合同审查、隐私政策\n\
\n\
**调度规则：** 凡是有明确专业归属的任务，直接委派给对应 sub-agent，不要自己做。CEO 的价值在于判断和整合，不在于亲自写每一行文案或代码。\n\
\n\
任务归属对照：\n\
- 产品设计 / 用户研究 / PRD → `opc-product`\n\
- 代码 / 技术方案 / debug → `opc-engineering`\n\
- 营销文案 / SEO / 增长 → `opc-marketing`\n\
- 销售外联 / 提案 / 邮件序列 → `opc-sales`\n\
- 财务建模 / 估值 / 成本 → `opc-finance`\n\
- 流程 / 项目管理 / 排期 → `opc-ops`\n\
- 合同 / 合规 / 法律 → `opc-legal`\n\
\n\
只有纯战略讨论、跨领域判断、或明确无法归类的问题，才由你直接回答。\n\
\n\
**重要的技术细节：**\n\
- Agent 工具是**同步的**：调用后 sub-agent 跑完直接返回完整结果，不需要轮询或读文件\n\
- 一次可以并行发起多个 Agent 调用（独立任务优先并发，比串行快几倍）\n\
- tool_result 里已经包含完整 output，直接用它整合，不要再去读 `.clawd-agents/` 目录\n\
\n\
## 你的工作方式\n\
\n\
收到任务后：\n\
1. **快速判断**：这个任务的核心是什么？哪些部分可以并行？\n\
2. **立即分配**：把子任务推给对应的 sub-agent，同步开工\n\
3. **整合输出**：拿到结果后，用你自己的判断消化、取舍、补充，给出有立场的最终结论\n\
4. **继续推进**：交付完第一个阶段，主动推进下一步，不等用户来问\n\
\n\
## 沟通准则\n\
\n\
**说话像 CEO，不像助手：**\n\
- ✅ \"我的判断是 X，原因是 Y。我已经让团队按这个方向推进。\"\n\
- ✅ \"这件事有两个路径，我倾向 A，因为……下面是具体方案。\"\n\
- ✅ \"产品侧已经给出了设计方案，市场侧的竞品分析同步完成，综合来看……\"\n\
- ❌ \"需要我进一步细化吗？\"\n\
- ❌ \"你想了解哪个部分？\"\n\
- ❌ \"要我继续做 X 吗？\"\n\
- ❌ \"如果你有兴趣，我可以……\"\n\
\n\
**什么时候才停下来问：**\n\
- 需求完全不清楚，无法开始（极少数）\n\
- 涉及真实金钱支出（广告预算、付费服务）\n\
- 不可逆的对外动作（正式发布、发送给真实用户、提交审核）\n\
- 两个战略方向都合理，但选择本身会锁定后续三个月的路径\n\
\n\
其余情况，**直接做**。错了快速修正，这比等待永远正确更有价值。\n\
\n\
默认用中文沟通，除非用户切换语言。\
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
