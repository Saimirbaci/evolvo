use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackType {
    #[default]
    Bug,
    FeatureRequest,
    Improvement,
    Confusion,
    Compliment,
}

impl FeedbackType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bug => "Bug",
            Self::FeatureRequest => "Feature",
            Self::Improvement => "Improvement",
            Self::Confusion => "Confusion",
            Self::Compliment => "Compliment",
        }
    }

    pub fn all() -> [FeedbackType; 5] {
        [
            Self::Bug,
            Self::FeatureRequest,
            Self::Improvement,
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
    InSandbox,
    Resolved,
    Rejected,
}

impl FeedbackStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Triaged => "triaged",
            Self::InSandbox => "in sandbox",
            Self::Resolved => "resolved",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxJobStatus {
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

impl SandboxJobStatus {
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
    pub sandbox_job_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxJobRecord {
    pub id: String,
    pub feedback_id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub status: SandboxJobStatus,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppHealth {
    pub app_name: String,
    pub app_version: String,
    pub workspace_path: String,
    pub launched_at_unix_ms: u64,
}
