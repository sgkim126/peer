use serde::{Deserialize, Serialize};

use crate::cache::{CacheKey, CacheStore};
use crate::llm::context::{
    ReviewCommentThread, ReviewContext, ReviewContextInput, ReviewThreadComment,
};
use crate::llm::provider::{
    ConversationTurn, LlmCallError, LlmOutputMode, LlmProvider, LlmRequest, LlmResponse, RawUsage,
};

const REVIEW_CONTEXT_SUMMARY_TOOL: &str = "review_context_summary";
// TODO: make it configurable
const BODY_COMPRESSION_THRESHOLD_CHARS: usize = 800;
// TODO: make it configurable
const COMMENTS_COMPRESSION_THRESHOLD_CHARS: usize = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewContextSummaryKind {
    Body,
    CommentThread,
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

    fn comments(comments: &[ReviewCommentThread]) -> Self {
        if let [thread] = comments {
            return Self::comment_thread(1, thread);
        }

        Self {
            kind: ReviewContextSummaryKind::CommentThread,
            content: format_comment_threads(comments),
        }
    }

    fn comment_thread(index: usize, thread: &ReviewCommentThread) -> Self {
        Self {
            kind: ReviewContextSummaryKind::CommentThread,
            content: format_comment_thread(index, thread),
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
pub struct PreparedReviewContext {
    pub context: ReviewContext,
    pub usage: RawUsage,
}

#[derive(Debug, Clone, PartialEq)]
struct ReviewContextSummaryOutput {
    summary: String,
    usage: RawUsage,
}

#[derive(Debug, Serialize)]
struct ReviewContextSummaryCacheParams {
    conversation: Vec<ReviewContextSummaryCacheTurn>,
}

#[derive(Debug, Serialize)]
struct ReviewContextSummaryCacheTurn {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReviewContextSummaryCacheValue {
    summary: String,
}

fn system_prompt(kind: ReviewContextSummaryKind) -> &'static str {
    match kind {
        ReviewContextSummaryKind::Body => {
            "Summarize the pull request body for a code review agent. Preserve the author's intent, stated risks, testing notes, and review instructions. Return only the summary."
        }
        ReviewContextSummaryKind::CommentThread => {
            "Summarize one pull request comment thread for a code review agent. Preserve unresolved concerns, requested changes, affected commits, and file locations. Return only the summary."
        }
    }
}

fn user_prompt(kind: ReviewContextSummaryKind, content: &str) -> String {
    match kind {
        ReviewContextSummaryKind::Body => {
            format!("Pull request body:\n{content}")
        }
        ReviewContextSummaryKind::CommentThread => {
            format!("Pull request comment thread:\n{content}")
        }
    }
}

async fn summarize_body<P>(
    provider: &P,
    provider_name: &str,
    model: &str,
    body: &str,
    cache: &CacheStore,
) -> Result<ReviewContextSummaryOutput, LlmCallError>
where
    P: LlmProvider,
{
    summarize_cached(
        provider,
        provider_name,
        model,
        ReviewContextSummaryInput::body(body),
        cache,
    )
    .await
}

async fn summarize_cached<P>(
    provider: &P,
    provider_name: &str,
    model: &str,
    input: ReviewContextSummaryInput,
    cache: &CacheStore,
) -> Result<ReviewContextSummaryOutput, LlmCallError>
where
    P: LlmProvider,
{
    let cache_key = cache_key(provider_name, model, &input)?;
    if let Ok(Some(value)) = cache.read_json::<ReviewContextSummaryCacheValue>(&cache_key) {
        return Ok(ReviewContextSummaryOutput {
            summary: value.summary,
            usage: RawUsage::default(),
        });
    }

    let output = summarize_impl(provider, model, input).await?;
    let value = ReviewContextSummaryCacheValue {
        summary: output.summary.clone(),
    };
    let _ = cache.write_json(&cache_key, &value);

    Ok(output)
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

pub async fn prepare_review_context<P>(
    provider: &P,
    provider_name: &str,
    model: &str,
    input: ReviewContextInput,
    cache: &CacheStore,
) -> Result<PreparedReviewContext, LlmCallError>
where
    P: LlmProvider,
{
    let ReviewContextInput {
        title,
        body,
        comments,
    } = input;
    let mut usage = RawUsage::default();
    let body_summary = if let Some(body) = body {
        if should_compress(&body, BODY_COMPRESSION_THRESHOLD_CHARS) {
            let output = summarize_body(provider, provider_name, model, &body, cache).await?;
            usage += output.usage;
            Some(output.summary)
        } else {
            Some(body)
        }
    } else {
        None
    };
    let comments_summary = if comments.is_empty() {
        None
    } else {
        let comments_input = ReviewContextSummaryInput::comments(&comments);
        if should_compress(
            &comments_input.content,
            COMMENTS_COMPRESSION_THRESHOLD_CHARS,
        ) {
            let output =
                summarize_cached(provider, provider_name, model, comments_input, cache).await?;
            usage += output.usage;
            Some(output.summary)
        } else {
            Some(comments_input.content)
        }
    };

    Ok(PreparedReviewContext {
        context: ReviewContext {
            title,
            body_summary,
            comments_summary,
        },
        usage,
    })
}

fn cache_key(
    provider_name: &str,
    model: &str,
    input: &ReviewContextSummaryInput,
) -> Result<CacheKey, LlmCallError> {
    let params = ReviewContextSummaryCacheParams {
        conversation: cache_conversation(input),
    };
    CacheKey::from_params(REVIEW_CONTEXT_SUMMARY_TOOL, provider_name, model, &params).map_err(
        |source| LlmCallError::Permanent {
            message: "failed to hash review context summary cache key".to_string(),
            source: Box::new(source),
        },
    )
}

fn cache_conversation(input: &ReviewContextSummaryInput) -> Vec<ReviewContextSummaryCacheTurn> {
    input
        .prompts()
        .into_iter()
        .map(|turn| match turn {
            ConversationTurn::System(content) => ReviewContextSummaryCacheTurn {
                role: "system",
                content,
            },
            ConversationTurn::User(content) => ReviewContextSummaryCacheTurn {
                role: "user",
                content,
            },
            _ => unreachable!("review context summary prompts only contain system and user turns"),
        })
        .collect()
}

fn should_compress(content: &str, threshold_chars: usize) -> bool {
    content.chars().count() > threshold_chars
}

fn format_comment_threads(threads: &[ReviewCommentThread]) -> String {
    threads
        .iter()
        .enumerate()
        .map(|(index, thread)| format_comment_thread(index + 1, thread))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_comment_thread(index: usize, thread: &ReviewCommentThread) -> String {
    let mut output = format!("Thread {index}:");

    if let Some(commit) = &thread.commit {
        output.push_str("\nCommit: ");
        output.push_str(commit.as_ref());
    }
    if let Some(location) = &thread.location {
        output.push_str("\nLocation: ");
        output.push_str(&location.path);
        output.push(':');
        output.push_str(&location.line.to_string());
    }

    for (comment_index, comment) in thread.comments.iter().enumerate() {
        output.push('\n');
        output.push_str(&format_thread_comment(comment_index + 1, comment));
    }

    output
}

fn format_thread_comment(index: usize, comment: &ReviewThreadComment) -> String {
    let mut output = format!("Comment {index} by {}:", comment.author);
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

    fn cache_store() -> (tempfile::TempDir, CacheStore) {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CacheStore::new(tmp.path().join("cache"), crate::console::Console::default());
        (tmp, cache)
    }

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
    fn builds_comment_thread_summary_prompts() {
        let thread = ReviewCommentThread {
            commit: Some(CommitHash::new("abc1234").unwrap()),
            location: Some(ReviewCommentLocation {
                path: "src/lib.rs".to_string(),
                line: 42,
            }),
            comments: vec![
                ReviewThreadComment {
                    author: "alice".to_string(),
                    body: "Please cover this branch.".to_string(),
                },
                ReviewThreadComment {
                    author: "bob".to_string(),
                    body: "Fixed in the latest push.".to_string(),
                },
            ],
        };
        let input = ReviewContextSummaryInput::comment_thread(1, &thread);

        assert_eq!(input.kind, ReviewContextSummaryKind::CommentThread);
        assert_eq!(
            input.content,
            "Thread 1:\nCommit: abc1234\nLocation: src/lib.rs:42\nComment 1 by alice:\nBody:\nPlease cover this branch.\nComment 2 by bob:\nBody:\nFixed in the latest push."
        );
        assert_eq!(
            input.prompts(),
            vec![
                ConversationTurn::System(
                    system_prompt(ReviewContextSummaryKind::CommentThread).to_string()
                ),
                ConversationTurn::User(
                    "Pull request comment thread:\nThread 1:\nCommit: abc1234\nLocation: src/lib.rs:42\nComment 1 by alice:\nBody:\nPlease cover this branch.\nComment 2 by bob:\nBody:\nFixed in the latest push.".to_string()
                ),
            ]
        );
    }

    #[test]
    fn formats_comment_threads_for_context_output() {
        let output = format_comment_threads(&[
            ReviewCommentThread {
                commit: Some(CommitHash::new("abc1234").unwrap()),
                location: Some(ReviewCommentLocation {
                    path: "src/lib.rs".to_string(),
                    line: 42,
                }),
                comments: vec![ReviewThreadComment {
                    author: "alice".to_string(),
                    body: "Please cover this branch.".to_string(),
                }],
            },
            ReviewCommentThread {
                commit: None,
                location: None,
                comments: vec![ReviewThreadComment {
                    author: "carol".to_string(),
                    body: "This looks resolved.".to_string(),
                }],
            },
        ]);

        assert_eq!(
            output,
            "Thread 1:\nCommit: abc1234\nLocation: src/lib.rs:42\nComment 1 by alice:\nBody:\nPlease cover this branch.\n\nThread 2:\nComment 1 by carol:\nBody:\nThis looks resolved."
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
        let (_tmp, cache) = cache_store();

        let output = summarize_body(
            &provider,
            "test-provider",
            "test-model",
            "Long PR body",
            &cache,
        )
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
        let (_tmp, cache) = cache_store();

        let error = summarize_body(&provider, "test-provider", "test-model", "body", &cache)
            .await
            .unwrap_err();

        assert!(matches!(error, LlmCallError::Permanent { .. }));
        assert!(
            error
                .to_string()
                .contains("review context summary was expected")
        );
    }

    #[tokio::test]
    async fn prepares_title_only_context_without_llm_call() {
        let provider = MockProvider::default();
        let (_tmp, cache) = cache_store();

        let prepared = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: Some("Add parser".to_string()),
                body: None,
                comments: Vec::new(),
            },
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(
            prepared,
            PreparedReviewContext {
                context: ReviewContext {
                    title: Some("Add parser".to_string()),
                    body_summary: None,
                    comments_summary: None,
                },
                usage: RawUsage::default(),
            }
        );
        assert!(provider.requests().is_empty());
    }

    #[tokio::test]
    async fn prepares_small_body_and_comments_without_llm_call() {
        let provider = MockProvider::default();
        let (_tmp, cache) = cache_store();
        let comments = vec![ReviewCommentThread {
            commit: None,
            location: None,
            comments: vec![ReviewThreadComment {
                author: "alice".to_string(),
                body: "Please cover this branch.".to_string(),
            }],
        }];

        let prepared = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: Some("Add parser".to_string()),
                body: Some("Short PR body".to_string()),
                comments: comments.clone(),
            },
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(
            prepared,
            PreparedReviewContext {
                context: ReviewContext {
                    title: Some("Add parser".to_string()),
                    body_summary: Some("Short PR body".to_string()),
                    comments_summary: Some(format_comment_threads(&comments)),
                },
                usage: RawUsage::default(),
            }
        );
        assert!(provider.requests().is_empty());
    }

    #[tokio::test]
    async fn prepares_context_with_body_and_comments_summaries() {
        let provider = MockProvider::new([
            Ok(LlmCallResult {
                response: LlmResponse::Text("body summary".to_string()),
                usage: RawUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            }),
            Ok(LlmCallResult {
                response: LlmResponse::Text("comments summary".to_string()),
                usage: RawUsage {
                    input_tokens: 20,
                    output_tokens: 8,
                },
            }),
        ]);
        let (_tmp, cache) = cache_store();

        let prepared = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: Some("Add parser".to_string()),
                body: Some("x".repeat(BODY_COMPRESSION_THRESHOLD_CHARS + 1)),
                comments: vec![ReviewCommentThread {
                    commit: None,
                    location: None,
                    comments: vec![ReviewThreadComment {
                        author: "alice".to_string(),
                        body: "y".repeat(COMMENTS_COMPRESSION_THRESHOLD_CHARS + 1),
                    }],
                }],
            },
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(
            prepared,
            PreparedReviewContext {
                context: ReviewContext {
                    title: Some("Add parser".to_string()),
                    body_summary: Some("body summary".to_string()),
                    comments_summary: Some("comments summary".to_string()),
                },
                usage: RawUsage {
                    input_tokens: 30,
                    output_tokens: 13,
                },
            }
        );
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| request.output_mode.clone())
                .collect::<Vec<_>>(),
            vec![RecordedLlmOutputMode::Text, RecordedLlmOutputMode::Text]
        );
    }

    #[tokio::test]
    async fn prepare_context_propagates_summary_error() {
        let provider = MockProvider::new([Err(LlmCallError::Transient {
            message: "timeout".to_string(),
            source: Box::new(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
        })]);
        let (_tmp, cache) = cache_store();

        let error = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: None,
                body: Some("x".repeat(BODY_COMPRESSION_THRESHOLD_CHARS + 1)),
                comments: Vec::new(),
            },
            &cache,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, LlmCallError::Transient { .. }));
    }

    #[tokio::test]
    async fn uses_cached_body_summary_without_llm_call() {
        let (_tmp, cache) = cache_store();
        let body = "x".repeat(BODY_COMPRESSION_THRESHOLD_CHARS + 1);
        let input = ReviewContextSummaryInput::body(&body);
        let key = cache_key("test-provider", "test-model", &input).unwrap();
        cache
            .write_json(
                &key,
                &ReviewContextSummaryCacheValue {
                    summary: "cached body summary".to_string(),
                },
            )
            .unwrap();
        let provider = MockProvider::default();

        let prepared = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: None,
                body: Some(body),
                comments: Vec::new(),
            },
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(
            prepared.context.body_summary,
            Some("cached body summary".to_string())
        );
        assert_eq!(prepared.usage, RawUsage::default());
        assert!(provider.requests().is_empty());
    }

    #[tokio::test]
    async fn writes_body_summary_to_cache_on_miss() {
        let (_tmp, cache) = cache_store();
        let body = "x".repeat(BODY_COMPRESSION_THRESHOLD_CHARS + 1);
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::Text("fresh body summary".to_string()),
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        })]);

        let prepared = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: None,
                body: Some(body.clone()),
                comments: Vec::new(),
            },
            &cache,
        )
        .await
        .unwrap();

        let key = cache_key(
            "test-provider",
            "test-model",
            &ReviewContextSummaryInput::body(body),
        )
        .unwrap();
        let cached = cache
            .read_json::<ReviewContextSummaryCacheValue>(&key)
            .unwrap()
            .unwrap();
        assert_eq!(
            prepared.context.body_summary,
            Some("fresh body summary".to_string())
        );
        assert_eq!(prepared.usage.input_tokens, 10);
        assert_eq!(cached.summary, "fresh body summary");
        assert_eq!(provider.requests().len(), 1);
    }

    #[tokio::test]
    async fn treats_corrupted_cache_as_miss() {
        let (_tmp, cache) = cache_store();
        let body = "x".repeat(BODY_COMPRESSION_THRESHOLD_CHARS + 1);
        let key = cache_key(
            "test-provider",
            "test-model",
            &ReviewContextSummaryInput::body(&body),
        )
        .unwrap();
        let path = cache.path_for(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "not json").unwrap();
        let provider = MockProvider::new([Ok(LlmCallResult {
            response: LlmResponse::Text("fresh summary".to_string()),
            usage: RawUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        })]);

        let prepared = prepare_review_context(
            &provider,
            "test-provider",
            "test-model",
            ReviewContextInput {
                title: None,
                body: Some(body),
                comments: Vec::new(),
            },
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(
            prepared.context.body_summary,
            Some("fresh summary".to_string())
        );
        assert_eq!(prepared.usage.input_tokens, 10);
        assert_eq!(provider.requests().len(), 1);
    }
}
