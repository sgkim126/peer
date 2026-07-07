#[cfg(test)]
use crate::llm::context::ReviewComment;
use crate::llm::provider::{
    ConversationTurn, LlmCallError, LlmOutputMode, LlmProvider, LlmRequest, LlmResponse, RawUsage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum ReviewContextSummaryKind {
    Body,
    Comments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewContextSummaryInput {
    kind: ReviewContextSummaryKind,
    content: String,
}

impl ReviewContextSummaryInput {
    fn body(body: impl Into<String>) -> Self {
        Self {
            kind: ReviewContextSummaryKind::Body,
            content: body.into(),
        }
    }

    #[cfg(test)]
    fn comments(comments: &[ReviewComment]) -> Self {
        Self {
            kind: ReviewContextSummaryKind::Comments,
            content: format_comments(comments),
        }
    }

    fn prompts(&self) -> Vec<ConversationTurn> {
        vec![
            ConversationTurn::System(system_prompt(self.kind).to_string()),
            ConversationTurn::User(user_prompt(self.kind, &self.content)),
        ]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewContextSummaryOutput {
    pub summary: String,
    pub usage: RawUsage,
}

fn system_prompt(kind: ReviewContextSummaryKind) -> &'static str {
    match kind {
        ReviewContextSummaryKind::Body => {
            "Summarize the pull request body for a code review agent. Preserve the author's intent, stated risks, testing notes, and review instructions. Return only the summary."
        }
        ReviewContextSummaryKind::Comments => {
            "Summarize pull request comments for a code review agent. Preserve unresolved concerns, requested changes, affected commits, and file locations. Return only the summary."
        }
    }
}

fn user_prompt(kind: ReviewContextSummaryKind, content: &str) -> String {
    match kind {
        ReviewContextSummaryKind::Body => {
            format!("Pull request body:\n{content}")
        }
        ReviewContextSummaryKind::Comments => {
            format!("Pull request comments:\n{content}")
        }
    }
}

#[allow(dead_code)]
async fn summarize_body<P>(
    provider: &P,
    model: &str,
    body: &str,
) -> Result<ReviewContextSummaryOutput, LlmCallError>
where
    P: LlmProvider,
{
    summarize_impl(provider, model, ReviewContextSummaryInput::body(body)).await
}

async fn summarize_impl<P>(
    provider: &P,
    model: &str,
    input: ReviewContextSummaryInput,
) -> Result<ReviewContextSummaryOutput, LlmCallError>
where
    P: LlmProvider,
{
    let prompts = input.prompts();
    let result = provider
        .send(LlmRequest {
            model,
            conversation: &prompts,
            output_mode: LlmOutputMode::Text,
        })
        .await?;

    let LlmResponse::Text(summary) = result.response else {
        return Err(LlmCallError::Permanent {
            message: "LLM returned non-text response while review context summary was expected"
                .to_string(),
            source: Box::new(ReviewContextSummaryError::UnexpectedResponse),
        });
    };

    Ok(ReviewContextSummaryOutput {
        summary,
        usage: result.usage,
    })
}

#[derive(Debug)]
enum ReviewContextSummaryError {
    UnexpectedResponse,
}

impl std::error::Error for ReviewContextSummaryError {}

impl std::fmt::Display for ReviewContextSummaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedResponse => f.write_str("unexpected review context summary response"),
        }
    }
}

#[cfg(test)]
fn format_comments(comments: &[ReviewComment]) -> String {
    comments
        .iter()
        .enumerate()
        .map(|(index, comment)| format_comment(index + 1, comment))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
fn format_comment(index: usize, comment: &ReviewComment) -> String {
    let mut output = format!("Comment {index}:");

    if let Some(commit) = &comment.commit {
        output.push_str("\nCommit: ");
        output.push_str(commit.as_ref());
    }
    if let Some(location) = &comment.location {
        output.push_str("\nLocation: ");
        output.push_str(&location.path);
        output.push(':');
        output.push_str(&location.line.to_string());
    }

    output.push_str("\nBody:\n");
    output.push_str(&comment.body);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitHash;
    use crate::llm::context::ReviewCommentLocation;
    use crate::llm::provider::{LlmCallResult, ToolCall};
    use crate::llm::test_support::{MockProvider, RecordedLlmOutputMode};

    #[test]
    fn builds_body_summary_prompts() {
        let input = ReviewContextSummaryInput::body("Fixes the parser.");

        assert_eq!(input.kind, ReviewContextSummaryKind::Body);
        assert_eq!(input.content, "Fixes the parser.");
        assert_eq!(
            input.prompts(),
            vec![
                ConversationTurn::System(system_prompt(ReviewContextSummaryKind::Body).to_string()),
                ConversationTurn::User("Pull request body:\nFixes the parser.".to_string()),
            ]
        );
    }

    #[test]
    fn formats_comments_with_optional_metadata() {
        let input = ReviewContextSummaryInput::comments(&[
            ReviewComment {
                body: "Please cover this branch.".to_string(),
                commit: Some(CommitHash::new("abc1234").unwrap()),
                location: Some(ReviewCommentLocation {
                    path: "src/lib.rs".to_string(),
                    line: 42,
                }),
            },
            ReviewComment {
                body: "This looks resolved.".to_string(),
                commit: None,
                location: None,
            },
        ]);

        assert_eq!(input.kind, ReviewContextSummaryKind::Comments);
        assert_eq!(
            input.content,
            "Comment 1:\nCommit: abc1234\nLocation: src/lib.rs:42\nBody:\nPlease cover this branch.\n\nComment 2:\nBody:\nThis looks resolved."
        );
    }

    #[tokio::test]
    async fn summarize_body_sends_text_request() {
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::Text("summary".to_string()),
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        })]);

        let output = summarize_body(&provider, "test-model", "Long PR body")
            .await
            .unwrap();

        assert_eq!(
            output,
            ReviewContextSummaryOutput {
                summary: "summary".to_string(),
                usage: RawUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            }
        );
        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, "test-model");
        assert_eq!(requests[0].output_mode, RecordedLlmOutputMode::Text);
        assert_eq!(
            requests[0].conversation,
            vec![
                ConversationTurn::System(system_prompt(ReviewContextSummaryKind::Body).to_string()),
                ConversationTurn::User("Pull request body:\nLong PR body".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn summarize_body_rejects_non_text_response() {
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::ToolCalls(vec![ToolCall {
                id: "call-1".to_string(),
                name: "tool".to_string(),
                arguments: serde_json::json!({}),
            }]),
            usage: RawUsage::default(),
        })]);

        let error = summarize_body(&provider, "test-model", "body")
            .await
            .unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(
            error
                .to_string()
                .contains("review context summary was expected")
        );
    }
}
