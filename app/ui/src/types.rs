use serde::{Deserialize, Serialize};

/// Which CLI coding agent should run a given lineage job. Mirrors
/// `evolvo_desktop::types::AgentKind` on the backend.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    ClaudeCode,
    CodexCli,
    GeminiCli,
    OpenCode,
}

impl AgentKind {
    pub fn all() -> [AgentKind; 4] {
        [
            Self::ClaudeCode,
            Self::CodexCli,
            Self::GeminiCli,
            Self::OpenCode,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::CodexCli => "Codex",
            Self::GeminiCli => "Gemini",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::CodexCli => "codex",
            Self::GeminiCli => "gemini",
            Self::OpenCode => "opencode",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentAvailability {
    pub kind: AgentKind,
    pub binary: String,
    pub installed: bool,
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
    NewApp,
}

impl FeedbackType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bug => "Bug",
            Self::FeatureRequest => "Feature",
            Self::Improvement => "Improvement",
            Self::Confusion => "Confusion",
            Self::Compliment => "Compliment",
            Self::NewApp => "New App",
        }
    }

    pub fn all() -> [FeedbackType; 6] {
        [
            Self::NewApp,
            Self::FeatureRequest,
            Self::Improvement,
            Self::Bug,
            Self::Confusion,
            Self::Compliment,
        ]
    }
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

impl FeedbackStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Triaged => "triaged",
            Self::InLineage => "in lineage",
            Self::Resolved => "resolved",
            Self::Rejected => "rejected",
        }
    }
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
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Triaging => "triaging",
            Self::Planned => "planned",
            Self::Implementing => "implementing",
            Self::BuildReady => "build ready",
            Self::Merging => "merging",
            Self::Promoted => "promoted",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }

    pub fn can_approve(self) -> bool {
        matches!(self, Self::Pending | Self::Planned | Self::BuildReady)
    }

    pub fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Implementing)
    }

    pub fn can_run(self) -> bool {
        matches!(self, Self::BuildReady | Self::Merging | Self::Promoted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackRecord {
    pub id: String,
    pub feedback_type: FeedbackType,
    pub status: FeedbackStatus,
    #[serde(default)]
    pub page_route: String,
    #[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LineageJobRecord {
    pub id: String,
    pub feedback_id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub status: LineageJobStatus,
    #[serde(default)]
    pub notes: Vec<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default)]
    pub source_repo: Option<String>,
    #[serde(default)]
    pub iteration: u32,
    #[serde(default)]
    pub stages: Vec<StageState>,
    #[serde(default)]
    pub agent: AgentKind,
}

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

    pub fn icon(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::Running => "◐",
            Self::Validating => "◑",
            Self::Green => "●",
            Self::Failed => "✕",
            Self::Skipped => "–",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StageState {
    pub kind: StageKind,
    #[serde(default)]
    pub status: StageStatus,
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default)]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub finished_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub headline: Option<String>,
    #[serde(default)]
    pub report: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitFeedbackPayload {
    pub feedback_type: FeedbackType,
    pub page_route: String,
    pub feedback_text: String,
    pub annotations: Vec<serde_json::Value>,
    pub pasted_images_base64: Vec<String>,
    pub screenshot_base64: Option<String>,
    pub voice_base64: Option<String>,
    pub voice_mime_type: Option<String>,
    pub voice_transcript: Option<String>,
    pub window_width: u32,
    pub window_height: u32,
    #[serde(default)]
    pub agent: Option<AgentKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppHealth {
    pub app_name: String,
    pub app_version: String,
    pub workspace_path: String,
    pub launched_at_unix_ms: u64,
}
