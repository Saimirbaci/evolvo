pub mod agent;
pub mod bundle;
pub mod commands;
pub mod concurrency;
pub mod delegation;
pub mod lineage;
pub mod plan;
pub mod proc;
pub mod promotion;
pub mod runner;
pub mod stages;
pub mod state;
pub mod store;
pub mod types;
pub mod validate_cli;
pub mod validators;

pub use agent::{backend_for, AgentBackend};
pub use lineage::{LineageEngine, Transition};
pub use state::AppState;
pub use store::{default_workspace_root, Store, StoreError, WorkspaceLayout};
pub use types::{
    current_time_unix_ms, AgentAvailability, AgentKind, AppHealth, DelegationStageStat,
    DelegationSummary, EntityIdPayload, FeedbackRecord, FeedbackStatus, FeedbackType,
    LineageJobRecord, LineageJobStatus, LocalDelegateAvailability, LocalDelegateConfig,
    PreviewSummary, StageKind, StageState, StageStatus, SubmitFeedbackPayload,
};
