use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub fn current_time_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
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
    pub sandbox_job_id: Option<String>,
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
    pub fn can_approve(self) -> bool {
        matches!(self, Self::Pending | Self::Planned | Self::BuildReady)
    }

    /// Retry is meaningful when a prior run ended badly (Failed) or the job
    /// is stuck mid-Implementing because the underlying process crashed or
    /// the reviewer interrupted it. `can_retry` gates the UI's Retry button.
    pub fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Implementing)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxJobRecord {
    pub id: String,
    pub feedback_id: String,
    pub title: String,
    pub summary: String,
    pub status: SandboxJobStatus,
    #[serde(default)]
    pub notes: Vec<String>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    /// Set once the sandbox pipeline has forked the source repo into a
    /// worktree for this job. Stored as an absolute path string so the UI
    /// can display it; not used for path resolution on the backend.
    #[serde(default)]
    pub worktree_path: Option<String>,
    /// Branch name inside the source repo that the worktree was checked
    /// out to (e.g. `sandbox/job-1700000000000`).
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
    /// the first change ever applied to a fresh NoIDE shell; later iterations
    /// progressively narrow the agent's freedom. `0` means "not yet
    /// allocated" — old records that pre-date the counter deserialize to 0
    /// and are treated as iteration 1 when they run.
    #[serde(default)]
    pub iteration: u32,
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
    fn sandbox_can_approve() {
        assert!(SandboxJobStatus::Pending.can_approve());
        assert!(SandboxJobStatus::Planned.can_approve());
        assert!(SandboxJobStatus::BuildReady.can_approve());
        assert!(!SandboxJobStatus::Rejected.can_approve());
        assert!(!SandboxJobStatus::Promoted.can_approve());
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
            sandbox_job_id: None,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"feedbackType\":\"bug\""));
        assert!(json.contains("\"createdAtUnixMs\":1"));
        let back: FeedbackRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
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
