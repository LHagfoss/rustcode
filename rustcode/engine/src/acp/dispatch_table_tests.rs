use super::*;

#[test]
fn every_dispatched_method_matches_its_request_type() {
    assert!(
        <InitializeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("initialize"),
        "method mismatch for InitializeRequest"
    );
    assert!(
        <NewSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("session/new"),
        "method mismatch for NewSessionRequest"
    );
    assert!(
        <ListSessionsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "session/list"
        ),
        "method mismatch for ListSessionsRequest"
    );
    assert!(
        <ThreadDeleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/threads/delete"
        ),
        "method mismatch for ThreadDeleteRequest"
    );
    assert!(
        <presence::PresenceRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/presence"
        ),
        "method mismatch for presence::PresenceRequest"
    );
    assert!(
        <presence::SetPresenceRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/presence/set"
        ),
        "method mismatch for presence::SetPresenceRequest"
    );
    assert!(
        <settings::GetSettingsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/settings/get"
        ),
        "method mismatch for settings::GetSettingsRequest"
    );
    assert!(
        <settings::SetSettingsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/settings/set"
        ),
        "method mismatch for settings::SetSettingsRequest"
    );
    assert!(
        <rewind::RewindRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/rewind"
        ),
        "method mismatch for rewind::RewindRequest"
    );
    assert!(
        <session_move::MoveSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/session/move"
        ),
        "method mismatch for session_move::MoveSessionRequest"
    );
    assert!(<session_replay::ReplaySessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/replay"), "method mismatch for session_replay::ReplaySessionRequest");
    assert!(<session_transfer::ExportSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/export"), "method mismatch for session_transfer::ExportSessionRequest");
    assert!(<session_transfer::ImportSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/import"), "method mismatch for session_transfer::ImportSessionRequest");
    assert!(<session_transfer::RemoveSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/remove"), "method mismatch for session_transfer::RemoveSessionRequest");
    assert!(<session_repair::RepairSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/repair"), "method mismatch for session_repair::RepairSessionRequest");
    assert!(
        <usage::UsagePlansRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/usage/plans"
        ),
        "method mismatch for usage::UsagePlansRequest"
    );
    assert!(
        <usage::UsageSharedRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/usage/shared"
        ),
        "method mismatch for usage::UsageSharedRequest"
    );
    assert!(
        <usage::UsageLimitsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/usage/limits"
        ),
        "method mismatch for usage::UsageLimitsRequest"
    );
    assert!(
        <commands::CommandsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/commands"
        ),
        "method mismatch for commands::CommandsRequest"
    );
    assert!(
        <outside::OutsideSignInRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/signin"
        ),
        "method mismatch for outside::OutsideSignInRequest"
    );
    assert!(<outside::OutsidePasskeyAddRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/passkey/add"), "method mismatch for outside::OutsidePasskeyAddRequest");
    assert!(<outside::OutsidePasskeyRemoveRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/passkey/remove"), "method mismatch for outside::OutsidePasskeyRemoveRequest");
    assert!(<outside::OutsideWebSignOutRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/web/sign-out"), "method mismatch for outside::OutsideWebSignOutRequest");
    assert!(
        <outside::OutsideCodeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/code"
        ),
        "method mismatch for outside::OutsideCodeRequest"
    );
    assert!(
        <outside::OutsideListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/list"
        ),
        "method mismatch for outside::OutsideListRequest"
    );
    assert!(
        <outside::OutsideCreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/create"
        ),
        "method mismatch for outside::OutsideCreateRequest"
    );
    assert!(
        <outside::OutsideUpdateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/update"
        ),
        "method mismatch for outside::OutsideUpdateRequest"
    );
    assert!(
        <outside::OutsideRevokeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/revoke"
        ),
        "method mismatch for outside::OutsideRevokeRequest"
    );
    assert!(
        <outside::OutsideAnswerRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/outside/answer"
        ),
        "method mismatch for outside::OutsideAnswerRequest"
    );
    assert!(
        <data::DataUsageRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/usage"
        ),
        "method mismatch for data::DataUsageRequest"
    );
    assert!(
        <data::DataQueryRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/query"
        ),
        "method mismatch for data::DataQueryRequest"
    );
    assert!(
        <data::DataActivityRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/activity"
        ),
        "method mismatch for data::DataActivityRequest"
    );
    assert!(
        <data::DataLinksLoadRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/links/load"
        ),
        "method mismatch for data::DataLinksLoadRequest"
    );
    assert!(
        <data::DataLinksSaveRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/links/save"
        ),
        "method mismatch for data::DataLinksSaveRequest"
    );
    assert!(
        <data::DataReceiptsGetRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/receipts/get"
        ),
        "method mismatch for data::DataReceiptsGetRequest"
    );
    assert!(<project_relocate::RelocateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/project/relocate"), "method mismatch for project_relocate::RelocateRequest");
    assert!(
        <machine_exec::MachineExecRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/machine/exec"
        ),
        "method mismatch for machine_exec::MachineExecRequest"
    );
    assert!(<machine_update::MachineUpdateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/machine/update"), "method mismatch for machine_update::MachineUpdateRequest");
    assert!(
        <projects::ProjectReadRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/project/read"
        ),
        "method mismatch for projects::ProjectReadRequest"
    );
    assert!(
        <projects::ProjectBrowseRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/project/browse"
        ),
        "method mismatch for projects::ProjectBrowseRequest"
    );
    assert!(
        <projects::ProjectCreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/project/create"
        ),
        "method mismatch for projects::ProjectCreateRequest"
    );
    assert!(
        <projects::ProjectCloneRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/project/clone"
        ),
        "method mismatch for projects::ProjectCloneRequest"
    );
    assert!(<attachments::AttachmentReadRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/attachments/read"), "method mismatch for attachments::AttachmentReadRequest");
    assert!(<attachments::AttachmentUploadRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/attachments/upload"), "method mismatch for attachments::AttachmentUploadRequest");
    assert!(
        <memory::MemoryListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/memory/list"
        ),
        "method mismatch for memory::MemoryListRequest"
    );
    assert!(
        <memory::MemoryForgetRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/memory/forget"
        ),
        "method mismatch for memory::MemoryForgetRequest"
    );
    assert!(
        <machine_sync::MachineSyncRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/environment/sync"
        ),
        "method mismatch for machine_sync::MachineSyncRequest"
    );
    assert!(
        <loops::LoopsListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/list"
        ),
        "method mismatch for loops::LoopsListRequest"
    );
    assert!(
        <loops::LoopsSaveRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/save"
        ),
        "method mismatch for loops::LoopsSaveRequest"
    );
    assert!(
        <loops::LoopsSetPausedRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/set_paused"
        ),
        "method mismatch for loops::LoopsSetPausedRequest"
    );
    assert!(
        <loops::LoopsRunRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/run"
        ),
        "method mismatch for loops::LoopsRunRequest"
    );
    assert!(
        <loops::LoopsCancelRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/cancel"
        ),
        "method mismatch for loops::LoopsCancelRequest"
    );
    assert!(
        <loops::LoopsDeleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/delete"
        ),
        "method mismatch for loops::LoopsDeleteRequest"
    );
    assert!(
        <loops::LoopsStartDaemonRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/loops/start_daemon"
        ),
        "method mismatch for loops::LoopsStartDaemonRequest"
    );
    assert!(
        <scheduled::ScheduledListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/scheduled/list"
        ),
        "method mismatch for scheduled::ScheduledListRequest"
    );
    assert!(<scheduled::ScheduledCreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/scheduled/create"), "method mismatch for scheduled::ScheduledCreateRequest");
    assert!(<scheduled::ScheduledDeleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/scheduled/delete"), "method mismatch for scheduled::ScheduledDeleteRequest");
    assert!(
        <scheduled::ScheduledRunRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/scheduled/run"
        ),
        "method mismatch for scheduled::ScheduledRunRequest"
    );
    assert!(
        <mirror::MirrorRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/mirror"
        ),
        "method mismatch for mirror::MirrorRequest"
    );
    assert!(
        <telemetry::TelemetryRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/config/telemetry"
        ),
        "method mismatch for telemetry::TelemetryRequest"
    );
    assert!(
        <data::DataReceiptsPutRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/data/receipts/put"
        ),
        "method mismatch for data::DataReceiptsPutRequest"
    );
    assert!(
        <telemetry::SetTelemetryRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/config/telemetry/set"
        ),
        "method mismatch for telemetry::SetTelemetryRequest"
    );
    assert!(
        <agents::AgentsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/agents"
        ),
        "method mismatch for agents::AgentsRequest"
    );
    assert!(
        <mcp_app::McpAppRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/mcp/app"
        ),
        "method mismatch for mcp_app::McpAppRequest"
    );
    assert!(
        <steer::SteerRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/steer"
        ),
        "method mismatch for steer::SteerRequest"
    );
    assert!(
        <remote::RemoteStatusRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote"
        ),
        "method mismatch for remote::RemoteStatusRequest"
    );
    assert!(
        <remote::RemoteStartRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/start"
        ),
        "method mismatch for remote::RemoteStartRequest"
    );
    assert!(
        <remote::RemotePairRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/pair"
        ),
        "method mismatch for remote::RemotePairRequest"
    );
    assert!(
        <remote::RemoteStopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/stop"
        ),
        "method mismatch for remote::RemoteStopRequest"
    );
    assert!(
        <remote::RemoteAnswerRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/answer"
        ),
        "method mismatch for remote::RemoteAnswerRequest"
    );
    assert!(
        <remote::RemoteConnectCodeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/connect_code"
        ),
        "method mismatch for remote::RemoteConnectCodeRequest"
    );
    assert!(
        <remote::RemoteRevokeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/revoke"
        ),
        "method mismatch for remote::RemoteRevokeRequest"
    );
    assert!(
        <revoke_machine::RevokeMachineRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/remote/revoke_machine"
        ),
        "method mismatch for revoke_machine::RevokeMachineRequest"
    );
    assert!(
        <harnesses::HarnessesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/harnesses"
        ),
        "method mismatch for harnesses::HarnessesRequest"
    );
    assert!(
        <harnesses::HarnessModelsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/harnesses/models"
        ),
        "method mismatch for harnesses::HarnessModelsRequest"
    );
    assert!(
        <harnesses::SetHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/harnesses/set"
        ),
        "method mismatch for harnesses::SetHarnessRequest"
    );
    assert!(
        <harnesses::RemoveHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/harnesses/remove"
        ),
        "method mismatch for harnesses::RemoveHarnessRequest"
    );
    assert!(
        <harnesses::DefaultHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/harnesses/default"
        ),
        "method mismatch for harnesses::DefaultHarnessRequest"
    );
    assert!(<harnesses::RememberHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/remember"), "method mismatch for harnesses::RememberHarnessRequest");
    assert!(<harnesses::AddHarnessAccountRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/accounts/add"), "method mismatch for harnesses::AddHarnessAccountRequest");
    assert!(<harnesses::RemoveHarnessAccountRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/accounts/remove"), "method mismatch for harnesses::RemoveHarnessAccountRequest");
    assert!(<harnesses::CheckHarnessAccountsRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/accounts/check"), "method mismatch for harnesses::CheckHarnessAccountsRequest");
    assert!(
        <harnesses::SwitchHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/session/harness"
        ),
        "method mismatch for harnesses::SwitchHarnessRequest"
    );
    assert!(<harnesses::OnboardingCompleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/onboarding/complete"), "method mismatch for harnesses::OnboardingCompleteRequest");
    assert!(<capabilities::CapabilitiesRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities"), "method mismatch for capabilities::CapabilitiesRequest");
    assert!(<capabilities::SetCapabilityRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/set"), "method mismatch for capabilities::SetCapabilityRequest");
    assert!(<capabilities::AddMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/add"), "method mismatch for capabilities::AddMcpServerRequest");
    assert!(<capabilities::RemoveMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/remove"), "method mismatch for capabilities::RemoveMcpServerRequest");
    assert!(<capabilities::RestartMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/restart"), "method mismatch for capabilities::RestartMcpServerRequest");
    assert!(<capabilities::LogoutMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/logout"), "method mismatch for capabilities::LogoutMcpServerRequest");
    assert!(
        <capabilities::AddSkillRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/skills/add"
        ),
        "method mismatch for capabilities::AddSkillRequest"
    );
    assert!(
        <capabilities::GetSkillRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/skills/get"
        ),
        "method mismatch for capabilities::GetSkillRequest"
    );
    assert!(
        <crate::worktrees::CreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/worktrees/create"
        ),
        "method mismatch for crate::worktrees::CreateRequest"
    );
    assert!(
        <crate::worktrees::ListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/worktrees/list"
        ),
        "method mismatch for crate::worktrees::ListRequest"
    );
    assert!(
        <crate::worktrees::RemoveRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/worktrees/remove"
        ),
        "method mismatch for crate::worktrees::RemoveRequest"
    );
    assert!(<crate::text_generation::ConfigureRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/textGeneration/configure"), "method mismatch for crate::text_generation::ConfigureRequest");
    assert!(<crate::transcription::TranscribeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/voice/transcribe"), "method mismatch for crate::transcription::TranscribeRequest");
    assert!(<crate::text_generation::GenerateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/textGeneration/generate"), "method mismatch for crate::text_generation::GenerateRequest");
    assert!(
        <crate::git_actions::StateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/git/state"
        ),
        "method mismatch for crate::git_actions::StateRequest"
    );
    assert!(
        <crate::git_actions::RunRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/git/run"
        ),
        "method mismatch for crate::git_actions::RunRequest"
    );
    assert!(
        <crate::worktrees::StateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/worktrees/state"
        ),
        "method mismatch for crate::worktrees::StateRequest"
    );
    assert!(
        <crate::worktrees::HandoffRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/worktrees/handoff"
        ),
        "method mismatch for crate::worktrees::HandoffRequest"
    );
    assert!(
        <crate::thread_status::PinRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/session/pin"
        ),
        "method mismatch for crate::thread_status::PinRequest"
    );
    assert!(
        <permissions::PermissionsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/permissions"
        ),
        "method mismatch for permissions::PermissionsRequest"
    );
    assert!(<permissions::RevokePermissionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/permissions/revoke"), "method mismatch for permissions::RevokePermissionRequest");
    assert!(
        <themes::ThemesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/themes"
        ),
        "method mismatch for themes::ThemesRequest"
    );
    assert!(
        <themes::SetThemeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/themes/set"
        ),
        "method mismatch for themes::SetThemeRequest"
    );
    assert!(<crate::thread_status::SnoozeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/snooze"), "method mismatch for crate::thread_status::SnoozeRequest");
    assert!(<crate::thread_status::SetStatusRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/status"), "method mismatch for crate::thread_status::SetStatusRequest");
    assert!(<crate::thread_status::SetTitleRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/title"), "method mismatch for crate::thread_status::SetTitleRequest");
    assert!(<crate::thread_index::SearchRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/threads/search"), "method mismatch for crate::thread_index::SearchRequest");
    assert!(
        <crate::checkpoints::ListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/checkpoints/list"
        ),
        "method mismatch for crate::checkpoints::ListRequest"
    );
    assert!(
        <crate::checkpoints::DiffRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/checkpoints/diff"
        ),
        "method mismatch for crate::checkpoints::DiffRequest"
    );
    assert!(<crate::checkpoints::RestoreRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/checkpoints/restore"), "method mismatch for crate::checkpoints::RestoreRequest");
    assert!(<server_status::ServerStatusRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/server/status"), "method mismatch for server_status::ServerStatusRequest");
    assert!(
        <WarmRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/warm"),
        "method mismatch for WarmRequest"
    );
    assert!(
        <PullsRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/pulls"),
        "method mismatch for PullsRequest"
    );
    assert!(
        <PullViewRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/view"
        ),
        "method mismatch for PullViewRequest"
    );
    assert!(
        <PullBranchRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/branch"
        ),
        "method mismatch for PullBranchRequest"
    );
    assert!(
        <PullStateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/state"
        ),
        "method mismatch for PullStateRequest"
    );
    assert!(
        <PullMergeCancelRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/merge/cancel"
        ),
        "method mismatch for PullMergeCancelRequest"
    );
    assert!(
        <PullMergeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/merge"
        ),
        "method mismatch for PullMergeRequest"
    );
    assert!(
        <WorktreeCleanupRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/worktrees/cleanup"
        ),
        "method mismatch for WorktreeCleanupRequest"
    );
    assert!(
        <PullDiffRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/diff"
        ),
        "method mismatch for PullDiffRequest"
    );
    assert!(
        <PullReferencesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/references"
        ),
        "method mismatch for PullReferencesRequest"
    );
    assert!(
        <PullThreadRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/thread"
        ),
        "method mismatch for PullThreadRequest"
    );
    assert!(
        <PullReviewRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/review"
        ),
        "method mismatch for PullReviewRequest"
    );
    assert!(
        <PullWatchStartRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/watch/start"
        ),
        "method mismatch for PullWatchStartRequest"
    );
    assert!(
        <PullWatchStopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/watch/stop"
        ),
        "method mismatch for PullWatchStopRequest"
    );
    assert!(
        <PullWatchesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/pulls/watches"
        ),
        "method mismatch for PullWatchesRequest"
    );
    assert!(
        <workflows::WorkflowListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/list"
        ),
        "method mismatch for workflows::WorkflowListRequest"
    );
    assert!(
        <workflows::WorkflowStopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/stop"
        ),
        "method mismatch for workflows::WorkflowStopRequest"
    );
    assert!(
        <workflows::WorkflowSavedRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/saved"
        ),
        "method mismatch for workflows::WorkflowSavedRequest"
    );
    assert!(
        <workflows::WorkflowStartRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/start"
        ),
        "method mismatch for workflows::WorkflowStartRequest"
    );
    assert!(
        <workflows::WorkflowResumeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/resume"
        ),
        "method mismatch for workflows::WorkflowResumeRequest"
    );
    assert!(
        <workflows::WorkflowRetryRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/retry"
        ),
        "method mismatch for workflows::WorkflowRetryRequest"
    );
    assert!(
        <workflows::WorkflowWakeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/workflows/wake"
        ),
        "method mismatch for workflows::WorkflowWakeRequest"
    );
    assert!(
        <connections::ConnectionsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/connections"
        ),
        "method mismatch for connections::ConnectionsRequest"
    );
    assert!(
        <connections::GitHubLoginRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/connections/github/login"
        ),
        "method mismatch for connections::GitHubLoginRequest"
    );
    assert!(
        <connections::GitHubWaitRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/connections/github/wait"
        ),
        "method mismatch for connections::GitHubWaitRequest"
    );
    assert!(
        <connections::GitHubLogoutRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/connections/github/logout"
        ),
        "method mismatch for connections::GitHubLogoutRequest"
    );
    assert!(<connections::AccountRefreshRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/account/refresh"), "method mismatch for connections::AccountRefreshRequest");
    assert!(<connections::AccountLogoutRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/account/logout"), "method mismatch for connections::AccountLogoutRequest");
    assert!(
        <connections::GitHubCancelRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/connections/github/cancel"
        ),
        "method mismatch for connections::GitHubCancelRequest"
    );
    assert!(
        <LoadSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "session/load"
        ),
        "method mismatch for LoadSessionRequest"
    );
    assert!(
        <CloseSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "session/close"
        ),
        "method mismatch for CloseSessionRequest"
    );
    assert!(
        <SetSessionConfigOptionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "session/set_config_option"
        ),
        "method mismatch for SetSessionConfigOptionRequest"
    );
    assert!(
        <stop::StopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
            "_rustcode/session/stop"
        ),
        "method mismatch for stop::StopRequest"
    );
    assert!(
        <PromptRequest as agent_client_protocol::JsonRpcMessage>::matches_method("session/prompt"),
        "method mismatch for PromptRequest"
    );
    assert!(
        <CancelNotification as agent_client_protocol::JsonRpcMessage>::matches_method(
            "session/cancel"
        )
    );
}
