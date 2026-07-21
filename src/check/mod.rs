pub mod runner;

use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::agent::AgentRequest;
use crate::llm::result::CheckTarget;

#[expect(dead_code)]
pub trait CheckDefinition {
    fn name(&self) -> &'static str;
    fn target(&self) -> CheckTarget;
    fn expected_commits(&self) -> &[CommitHash];
    async fn agent_request(
        &self,
        extractor: &Extractor,
        model: &str,
    ) -> Result<AgentRequest, ExtractError>;
}
