use serde::Deserialize;

use crate::gh::{GhClient, GhInvocation};

use super::{CommandError, PullRequestInput, api};

#[derive(Debug, Deserialize)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct FeedbackComment {
    pub body: String,
    #[serde(rename = "html_url")]
    pub url: Option<String>,
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn list_review_feedback(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<FeedbackComment>, CommandError> {
    api::list(client, &format!("{}/comments", input.path())).await
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn list_conversation_feedback(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<FeedbackComment>, CommandError> {
    #[derive(Deserialize)]
    struct ConversationComments {
        comments: Vec<ConversationComment>,
    }

    #[derive(Deserialize)]
    struct ConversationComment {
        body: String,
        url: Option<String>,
    }

    let output = client
        .run(&GhInvocation {
            args: vec![
                "pr".into(),
                "view".into(),
                input.number.to_string(),
                "--repo".into(),
                input.repo_arg(),
                "--json".into(),
                "comments".into(),
            ],
            stdin: None,
        })
        .await?;
    // gh pr view completes all comment pages before emitting its JSON output.
    Ok(api::decode::<ConversationComments>(output, "gh pr view")?
        .comments
        .into_iter()
        .map(|comment| FeedbackComment {
            body: comment.body,
            url: comment.url,
        })
        .collect())
}
