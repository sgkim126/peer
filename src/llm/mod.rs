mod agent;
mod provider;
mod result;
mod tools;

#[cfg(test)]
mod test_support;

pub use self::agent::{Agent, AgentCheckpoint, AgentOutcome, AgentRequest};
use self::provider::LlmRequest;
pub use self::provider::{
    ConversationTurn, LlmCallError, LlmProvider, LlmResponse, LlmTransport, ProviderCreationError,
    ProviderKind, ProviderRuntime, RawUsage, ToolCall, ToolSpec,
};
pub use self::result::{
    CheckError, CheckOutput, CheckResult, CheckTarget, Finding, LlmModelUsage, LlmUsage, Severity,
};
pub use self::tools::{
    ExtractToolExecutor, ToolExecutionResult, ToolExecutor, get_changed_files, get_commit_diff,
    get_file_content, get_file_diff, grep, list_tree, request_clarification, submit_check_result,
};

#[cfg(test)]
pub use self::provider::{LlmCallResult, Request, Response};
#[cfg(test)]
pub use self::result::FileLocation;
#[cfg(test)]
pub use self::test_support::MockProvider;
#[cfg(test)]
use self::tools::ToolExecutionError;
