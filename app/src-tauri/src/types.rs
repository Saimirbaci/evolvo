use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub fn current_time_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Which CLI coding agent should run a given lineage job. Stored on the
/// `LineageJobRecord` (and optionally submitted via `SubmitFeedbackPayload`)
/// so that Retry / Resume use the same backend that enqueued the work. The
/// concrete spawn wiring lives in `agent.rs`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    ClaudeCode,
    CodexCli,
    GeminiCli,
    /// ForgeCode (forgecode.dev) — `forge -p <prompt>` runs a single
    /// non-interactive turn. The `open_code` alias keeps lineage jobs
    /// persisted by older builds (which ran the OpenCode CLI under the
    /// same slot) deserialisable; those records render as Forge in the
    /// UI even though they were originally executed by `opencode`.
    #[serde(alias = "open_code")]
    Forge,
}

impl AgentKind {
    pub fn all() -> [AgentKind; 4] {
        [
            Self::ClaudeCode,
            Self::CodexCli,
            Self::GeminiCli,
            Self::Forge,
        ]
    }

    /// Short human label used in the UI and in job notes.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::CodexCli => "Codex",
            Self::GeminiCli => "Gemini",
            Self::Forge => "Forge",
        }
    }

    /// Stable slug used in log filenames and metadata. Keep in sync with
    /// `agent::AgentBackend::log_filename`.
    pub fn slug(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::CodexCli => "codex",
            Self::GeminiCli => "gemini",
            Self::Forge => "forge",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackType {
    #[default]
    Bug,
    FeatureRequest,
    Improvement,
    Confusion,
    Compliment,
    /// The user wants the agent to build a **new app from scratch** on top
    /// of the Evolvo shell. Regardless of iteration number, a `NewApp` feedback
    /// unlocks full "bootstrap" latitude in the prompt — the agent should
    /// treat the existing code as scaffolding and produce the app the user
    /// described in the canvas + text + voice, preserving only the four
    /// product invariants (Feedback Overlay, Canvas per-page overlay, Inbox,
    /// Lineage pipeline).
    NewApp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackStatus {
    #[default]
    New,
    Triaged,
    InLineage,
    Resolved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRecord {
    pub id: String,
    pub feedback_type: FeedbackType,
    pub status: FeedbackStatus,
    pub page_route: String,
    pub feedback_text: String,
    #[serde(default)]
    pub annotations: Vec<serde_json::Value>,
    #[serde(default)]
    pub pasted_images: Vec<String>,
    pub screenshot_filename: Option<String>,
    pub voice_filename: Option<String>,
    pub voice_transcript: Option<String>,
    pub window_width: u32,
    pub window_height: u32,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub lineage_job_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LineageJobStatus {
    #[default]
    Pending,
    Triaging,
    Planned,
    Implementing,
    BuildReady,
    Merging,
    Promoted,
    Rejected,
    Failed,
}

impl LineageJobStatus {
    pub fn can_approve(self) -> bool {
        matches!(self, Self::Pending | Self::Planned | Self::BuildReady)
    }

    /// Retry is meaningful when a prior run ended badly (Failed) or the job
    /// is stuck mid-Implementing because the underlying process crashed or
    /// the reviewer interrupted it. `can_retry` gates the UI's Retry button.
    pub fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Implementing)
    }

    /// Launching the built iteration only makes sense once the agent has
    /// finished writing code to the worktree — i.e. `BuildReady` (default
    /// after a successful Advance) or `Promoted` (after a reviewer approved
    /// it). `Merging` counts too since the worktree is intact.
    pub fn can_run(self) -> bool {
        matches!(self, Self::BuildReady | Self::Merging | Self::Promoted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LineageJobRecord {
    pub id: String,
    pub feedback_id: String,
    pub title: String,
    pub summary: String,
    pub status: LineageJobStatus,
    #[serde(default)]
    pub notes: Vec<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    /// Set once the lineage pipeline has forked the source repo into a
    /// worktree for this job. Stored as an absolute path string so the UI
    /// can display it; not used for path resolution on the backend.
    #[serde(default)]
    pub worktree_path: Option<String>,
    /// Branch name inside the source repo that the worktree was checked
    /// out to (e.g. `lineage/job-1700000000000`).
    #[serde(default)]
    pub branch_name: Option<String>,
    /// Absolute path to the `claude.log` file that captures the agent's
    /// stdout + stderr. Useful for displaying a tail in the UI.
    #[serde(default)]
    pub log_path: Option<String>,
    /// Absolute path of the source repo that was forked. Helps reviewers
    /// understand where the worktree came from.
    #[serde(default)]
    pub source_repo: Option<String>,
    /// 1-indexed iteration number for the evolving meta-app. Iteration 1 is
    /// the first change ever applied to a fresh Evolvo shell; later iterations
    /// progressively narrow the agent's freedom. `0` means "not yet
    /// allocated" — old records that pre-date the counter deserialize to 0
    /// and are treated as iteration 1 when they run.
    #[serde(default)]
    pub iteration: u32,
    /// Multi-stage planner pipeline progress for NewApp iterations. Empty
    /// for classic single-session runs (bug fixes, small features). Each
    /// entry is appended when the corresponding stage starts and mutated
    /// in place as the stage progresses / finishes. Serialized default so
    /// older records without this field round-trip cleanly.
    #[serde(default)]
    pub stages: Vec<StageState>,
    /// Which CLI coding agent was selected to run this job. Defaults to
    /// `ClaudeCode` for pre-existing records that pre-date the multi-agent
    /// feature. Retry / Resume reuse this value so a failed Codex run never
    /// silently flips to Claude half-way through.
    #[serde(default)]
    pub agent: AgentKind,
}

/// Which stage of the multi-stage NewApp pipeline a `StageState` represents.
/// Plan stages are read-only (the Claude session writes only to `plan.json`);
/// impl stages write code to the worktree.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StageKind {
    BackendPlan,
    BackendImpl,
    FrontendPlan,
    FrontendImpl,
    E2EPlan,
    E2EImpl,
    FinalReview,
}

impl StageKind {
    pub fn slug(self) -> &'static str {
        match self {
            Self::BackendPlan => "backend_plan",
            Self::BackendImpl => "backend_impl",
            Self::FrontendPlan => "frontend_plan",
            Self::FrontendImpl => "frontend_impl",
            Self::E2EPlan => "e2e_plan",
            Self::E2EImpl => "e2e_impl",
            Self::FinalReview => "final_review",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::BackendPlan => "Backend plan",
            Self::BackendImpl => "Backend impl",
            Self::FrontendPlan => "Frontend plan",
            Self::FrontendImpl => "Frontend impl",
            Self::E2EPlan => "E2E plan",
            Self::E2EImpl => "E2E impl",
            Self::FinalReview => "Final review",
        }
    }

    pub fn is_planner(self) -> bool {
        matches!(self, Self::BackendPlan | Self::FrontendPlan | Self::E2EPlan)
    }

    /// Canonical order for the pipeline.
    pub fn pipeline() -> &'static [StageKind] {
        &[
            Self::BackendPlan,
            Self::BackendImpl,
            Self::FrontendPlan,
            Self::FrontendImpl,
            Self::E2EPlan,
            Self::E2EImpl,
            Self::FinalReview,
        ]
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    #[default]
    Pending,
    Running,
    Validating,
    Green,
    Failed,
    Skipped,
}

impl StageStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Validating => "validating",
            Self::Green => "green",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Green | Self::Failed | Self::Skipped)
    }
}

/// One stage of the NewApp pipeline with enough metadata for the UI to
/// render a live progress panel. Stored inside `LineageJobRecord.stages`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StageState {
    pub kind: StageKind,
    #[serde(default)]
    pub status: StageStatus,
    /// Absolute path to the per-stage `claude.log` (for planner / impl
    /// stages) or validator report (for the final review stage).
    #[serde(default)]
    pub log_path: Option<String>,
    /// Monotonic unix-ms timestamp when the stage first flipped to
    /// `Running`. `None` while still `Pending`.
    #[serde(default)]
    pub started_at_unix_ms: Option<u64>,
    /// Set when the stage reaches any terminal status (`Green`, `Failed`,
    /// `Skipped`).
    #[serde(default)]
    pub finished_at_unix_ms: Option<u64>,
    /// Short human-readable summary surfaced in the UI — the first error
    /// on failure, the validator headline on green, etc.
    #[serde(default)]
    pub headline: Option<String>,
    /// Structured validator output (counts, checks) — serialized to JSON
    /// so the UI can render a table without re-parsing free text.
    #[serde(default)]
    pub report: Option<serde_json::Value>,
}

impl StageState {
    pub fn pending(kind: StageKind) -> Self {
        Self {
            kind,
            status: StageStatus::Pending,
            log_path: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            headline: None,
            report: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubmitFeedbackPayload {
    pub feedback_type: FeedbackType,
    #[serde(default)]
    pub page_route: String,
    pub feedback_text: String,
    #[serde(default)]
    pub annotations: Vec<serde_json::Value>,
    #[serde(default)]
    pub pasted_images_base64: Vec<String>,
    pub screenshot_base64: Option<String>,
    pub voice_base64: Option<String>,
    pub voice_mime_type: Option<String>,
    pub voice_transcript: Option<String>,
    pub window_width: u32,
    pub window_height: u32,
    /// Optional explicit agent selection. `None` falls back to
    /// `AgentKind::default()` (Claude Code) so older UI builds keep working.
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// When true, `submit_feedback` immediately fires
    /// `start_implementation_run` after enqueuing the lineage job — the
    /// reviewer doesn't have to switch to the Lineage tab and click Evolve.
    /// Defaults to false on the wire so older UI builds keep behaving as
    /// before; the UI sets this from a per-submission checkbox that is
    /// pre-checked for `NewApp` feedback (where the user almost always
    /// wants the agent to start immediately).
    #[serde(default)]
    pub auto_evolve: bool,
}

/// Availability of a given agent CLI on the host. Returned by the
/// `list_available_agents` Tauri command so the UI can grey out agents the
/// user hasn't installed yet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentAvailability {
    pub kind: AgentKind,
    pub binary: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EntityIdPayload {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppHealth {
    pub app_name: String,
    pub app_version: String,
    pub workspace_path: String,
    pub launched_at_unix_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_type_round_trips() {
        let raw = serde_json::to_string(&FeedbackType::Bug).unwrap();
        assert_eq!(raw, "\"bug\"");
        let back: FeedbackType = serde_json::from_str("\"feature_request\"").unwrap();
        assert_eq!(back, FeedbackType::FeatureRequest);
    }

    #[test]
    fn lineage_can_approve() {
        assert!(LineageJobStatus::Pending.can_approve());
        assert!(LineageJobStatus::Planned.can_approve());
        assert!(LineageJobStatus::BuildReady.can_approve());
        assert!(!LineageJobStatus::Rejected.can_approve());
        assert!(!LineageJobStatus::Promoted.can_approve());
    }

    #[test]
    fn feedback_record_round_trips_camel_case() {
        let rec = FeedbackRecord {
            id: "fb-1".into(),
            feedback_type: FeedbackType::Bug,
            status: FeedbackStatus::New,
            page_route: "/".into(),
            feedback_text: "x".into(),
            annotations: vec![],
            pasted_images: vec![],
            screenshot_filename: None,
            voice_filename: None,
            voice_transcript: None,
            window_width: 800,
            window_height: 600,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            lineage_job_id: None,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"feedbackType\":\"bug\""));
        assert!(json.contains("\"createdAtUnixMs\":1"));
        let back: FeedbackRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn submit_payload_auto_evolve_defaults_false_and_round_trips() {
        // Older UI builds don't send the field; deserialise must default to
        // false so they keep the old "manual Evolve" behaviour.
        let json = r#"{
            "feedbackType":"bug",
            "feedbackText":"x",
            "windowWidth":1,
            "windowHeight":1,
            "screenshotBase64":null,
            "voiceBase64":null,
            "voiceMimeType":null,
            "voiceTranscript":null
        }"#;
        let p: SubmitFeedbackPayload = serde_json::from_str(json).unwrap();
        assert!(!p.auto_evolve, "auto_evolve must default to false");

        // And explicit `autoEvolve: true` rides through camel-cased.
        let json_on = r#"{
            "feedbackType":"new_app",
            "feedbackText":"build me a budget tracker",
            "windowWidth":1,
            "windowHeight":1,
            "screenshotBase64":null,
            "voiceBase64":null,
            "voiceMimeType":null,
            "voiceTranscript":null,
            "autoEvolve":true
        }"#;
        let p: SubmitFeedbackPayload = serde_json::from_str(json_on).unwrap();
        assert!(p.auto_evolve);
    }

    #[test]
    fn agent_kind_round_trips_snake_case() {
        let raw = serde_json::to_string(&AgentKind::ClaudeCode).unwrap();
        assert_eq!(raw, "\"claude_code\"");
        let back: AgentKind = serde_json::from_str("\"codex_cli\"").unwrap();
        assert_eq!(back, AgentKind::CodexCli);
        let back: AgentKind = serde_json::from_str("\"gemini_cli\"").unwrap();
        assert_eq!(back, AgentKind::GeminiCli);
        let back: AgentKind = serde_json::from_str("\"forge\"").unwrap();
        assert_eq!(back, AgentKind::Forge);
        // Legacy lineage jobs persisted by builds that called the slot
        // OpenCode must still load — the alias points the old slug at the
        // new variant rather than failing the deserialise.
        let legacy: AgentKind = serde_json::from_str("\"open_code\"").unwrap();
        assert_eq!(legacy, AgentKind::Forge);
    }

    #[test]
    fn lineage_job_record_defaults_agent_for_old_records() {
        // Old records written before the multi-agent feature won't have the
        // `agent` field; verify they deserialise as ClaudeCode.
        let json = r#"{
            "id": "job-1",
            "feedbackId": "fb-1",
            "title": "old",
            "summary": "",
            "status": "pending",
            "createdAtUnixMs": 0,
            "updatedAtUnixMs": 0
        }"#;
        let back: LineageJobRecord = serde_json::from_str(json).unwrap();
        assert_eq!(back.agent, AgentKind::ClaudeCode);
        assert_eq!(back.iteration, 0);
    }

    #[test]
    fn feedback_record_tolerates_extra_fields() {
        let json = r#"{
            "id": "fb-1",
            "feedbackType": "bug",
            "status": "new",
            "pageRoute": "/",
            "feedbackText": "",
            "windowWidth": 0,
            "windowHeight": 0,
            "createdAtUnixMs": 0,
            "updatedAtUnixMs": 0,
            "somethingWeNeverKnewAbout": "ignored"
        }"#;
        let back: FeedbackRecord = serde_json::from_str(json).unwrap();
        assert_eq!(back.id, "fb-1");
        assert!(back.annotations.is_empty());
        assert!(back.pasted_images.is_empty());
    }
}
