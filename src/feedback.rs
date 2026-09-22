use std::collections::{BTreeSet, HashSet};

use serde_json::{Value, json};

use crate::git::CommitHash;
use crate::render::{RenderDocument, RenderFinding, github, gitlab};
use crate::stage::{FileLocation, KnowledgeQuestion, StructuralRecommendation};

use crate::github::Repository;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Question,
    Recommendation,
    Finding,
}

pub struct Feedback {
    pub fingerprint: String,
    pub body: String,
    pub location: Option<FileLocation>,
    pub commit: Option<CommitHash>,
    kind: FeedbackKind,
}

pub struct PreparedReview {
    pub items: Vec<Feedback>,
    pub summary_fingerprint: Option<String>,
    parts: Option<github::DocumentParts>,
}

impl PreparedReview {
    pub fn new(document: &RenderDocument, repository: &Repository) -> Self {
        let parts = github::DocumentParts::new(document, &repository.to_string());
        Self::with_parts(document, parts)
    }

    pub fn for_gitlab(document: &RenderDocument, repository: &crate::gitlab::Repository) -> Self {
        let parts = gitlab::DocumentParts::for_gitlab(document, &repository.to_string());
        let mut prepared = Self::with_parts(document, parts);
        let related_commits = document
            .questions
            .iter()
            .map(|question| question.related_commits.as_slice())
            .chain(
                document
                    .recommendations
                    .iter()
                    .map(|recommendation| recommendation.related_commits.as_slice()),
            );
        for (item, commits) in prepared.items.iter_mut().zip(related_commits) {
            if item.commit.is_none() {
                item.commit = unique_commit(commits).cloned();
            }
        }
        prepared
    }

    fn with_parts(document: &RenderDocument, parts: github::DocumentParts) -> Self {
        let items: Vec<_> = document
            .questions
            .iter()
            .zip(&parts.questions)
            .map(|(question, body)| prepare_question(question, body.clone()))
            .chain(
                document
                    .recommendations
                    .iter()
                    .zip(&parts.recommendations)
                    .map(|(recommendation, body)| {
                        prepare_recommendation(recommendation, body.clone())
                    }),
            )
            .chain(
                document
                    .findings
                    .iter()
                    .zip(&parts.findings)
                    .map(|(finding, body)| prepare_finding(finding, body.clone())),
            )
            .collect();
        let parts = Some(parts);
        let summary_fingerprint = match &parts {
            Some(parts)
                if !parts.summary.is_empty()
                    || !parts.context.is_empty()
                    || !parts.stages.is_empty() =>
            {
                Some(fingerprint(
                    "summary",
                    json!({
                        "items": items.iter().map(|item| &item.fingerprint).collect::<BTreeSet<_>>(),
                        "review": github::summary_identity(document),
                    }),
                ))
            }
            _ => None,
        };
        Self {
            items,
            summary_fingerprint,
            parts,
        }
    }

    pub fn aggregate(&self, remaining: &[&Feedback], include_summary: bool) -> String {
        let body = |item: &&Feedback| format!("{}\n\n{}", item.body, marker(&item.fingerprint));
        let Some(mut parts) = self.parts.clone() else {
            return remaining.iter().map(body).collect::<Vec<_>>().join("\n\n");
        };
        parts.questions.clear();
        parts.recommendations.clear();
        parts.findings.clear();
        for item in remaining {
            let section = match item.kind {
                FeedbackKind::Question => &mut parts.questions,
                FeedbackKind::Recommendation => &mut parts.recommendations,
                FeedbackKind::Finding => &mut parts.findings,
            };
            section.push(body(item));
        }
        if !include_summary {
            parts.summary.clear();
            parts.context.clear();
            parts.stages.clear();
        }
        let mut output = parts.render();
        if include_summary && let Some(fingerprint) = &self.summary_fingerprint {
            output.push_str(&format!("\n\n{}", marker(fingerprint)));
        }
        output
    }
}

fn unique_commit(commits: &[CommitHash]) -> Option<&CommitHash> {
    let first = commits.first()?;
    commits
        .iter()
        .all(|commit| commit.matches(first))
        .then_some(first)
}

fn prepare_question(question: &KnowledgeQuestion, body: String) -> Feedback {
    Feedback {
        fingerprint: fingerprint(
            "question",
            json!({
                "category": question.category,
                "question": question.question,
                "evidence": question.evidence,
                "why_it_matters": question.why_it_matters,
                "location": question.location.as_ref().map(|location| &location.file),
            }),
        ),
        body,
        location: question
            .location
            .as_ref()
            .map(|location| location.file.clone()),
        commit: question
            .location
            .as_ref()
            .map(|location| location.commit.clone()),
        kind: FeedbackKind::Question,
    }
}

fn prepare_recommendation(recommendation: &StructuralRecommendation, body: String) -> Feedback {
    Feedback {
        fingerprint: fingerprint(
            "recommendation",
            json!({
                "kind": recommendation.kind,
                "message": recommendation.message,
                "rationale": recommendation.rationale,
            }),
        ),
        body,
        location: None,
        commit: None,
        kind: FeedbackKind::Recommendation,
    }
}

fn prepare_finding(finding: &RenderFinding, body: String) -> Feedback {
    Feedback {
        fingerprint: fingerprint(
            "finding",
            json!({
                "severity": finding.severity,
                "message": finding.message,
                "location": finding.location,
                "security": finding.security,
            }),
        ),
        body,
        location: finding.location.clone(),
        commit: Some(finding.commit.clone()),
        kind: FeedbackKind::Finding,
    }
}

fn fingerprint(kind: &str, identity: Value) -> String {
    let value = json!(["peer-review:v1", kind, identity]);
    blake3::hash(value.to_string().as_bytes())
        .to_hex()
        .to_string()
}

const MARKER_PREFIX: &str = "<!-- peer-review:v1:";
const MARKER_SUFFIX: &str = " -->";
pub fn marker(fingerprint: &str) -> String {
    format!("{MARKER_PREFIX}{fingerprint}{MARKER_SUFFIX}")
}

pub fn fingerprints(body: &str) -> HashSet<String> {
    body.lines()
        .filter_map(|line| {
            let hash = line
                .trim()
                .strip_prefix(MARKER_PREFIX)?
                .strip_suffix(MARKER_SUFFIX)?;
            (hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
            .then(|| hash.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(section: &str, value: Value, repo: &str) -> String {
        PreparedReview::new(
            &serde_json::from_value(json!({
                "ordered_commits": [],
                "stages": [],
                (section): [value]
            }))
            .unwrap(),
            &Repository::parse(repo).unwrap(),
        )
        .items[0]
            .fingerprint
            .clone()
    }

    fn finding() -> Value {
        json!({
            "commit": "abc1234",
            "severity": "high",
            "message": "Issue",
            "file": "src/main.rs",
            "line": 5
        })
    }

    fn question() -> Value {
        json!({
            "category": "rationale",
            "question": "Why?",
            "evidence": "Evidence",
            "why_it_matters": "Reason",
            "related_commits": ["abc1234"],
            "location": {
                "commit": "abc1234",
                "file": "src/main.rs",
                "line": 5
            }
        })
    }

    fn recommendation() -> Value {
        json!({
            "kind": "split_commit",
            "message": "Split this",
            "rationale": "Separate concerns",
            "related_commits": ["abc1234"]
        })
    }

    fn document_with_feedback(section: &str, feedback: Value) -> RenderDocument {
        serde_json::from_value(json!({
            "ordered_commits": ["abc1234", "def5678"],
            "stages": [],
            (section): [feedback],
        }))
        .unwrap()
    }

    fn gitlab_feedback(section: &str, value: Value) -> Feedback {
        let document = document_with_feedback(section, value);
        let prepared = PreparedReview::for_gitlab(
            &document,
            &crate::gitlab::Repository::parse("group/project").unwrap(),
        );
        assert_eq!(prepared.items.len(), 1);
        prepared.items.into_iter().next().unwrap()
    }

    #[test]
    fn finding_fingerprint_ignores_commit() {
        let mut value = finding();
        let original = identity("findings", value.clone(), "owner/repo");
        value["commit"] = json!("def5678");

        assert_eq!(identity("findings", value, "owner/repo"), original);
    }

    #[test]
    fn provider_rendering_preserves_fingerprints_and_original_commit() {
        let document = serde_json::from_value(json!({
            "ordered_commits": ["abc1234"],
            "stages": [],
            "findings": [finding()],
        }))
        .unwrap();
        let github = PreparedReview::new(&document, &Repository::parse("owner/repo").unwrap());
        let gitlab = PreparedReview::for_gitlab(
            &document,
            &crate::gitlab::Repository::parse("group/subgroup/project").unwrap(),
        );
        assert_eq!(github.items[0].fingerprint, gitlab.items[0].fingerprint);
        assert_eq!(gitlab.items[0].commit.as_ref().unwrap().as_ref(), "abc1234");
        assert!(gitlab.items[0].body.contains("https://gitlab.com/"));
        assert!(github.items[0].body.contains("https://github.com/"));
    }

    #[test]
    fn questions_keep_location_and_commit_with_a_matching_related_commit() {
        let question: KnowledgeQuestion = serde_json::from_value(question()).unwrap();

        let feedback = prepare_question(&question, "Question".into());

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
        let location = feedback.location.unwrap();
        assert_eq!(location.file, "src/main.rs");
        assert_eq!(location.line, Some(5));
    }

    #[test]
    fn questions_keep_location_and_commit_with_multiple_related_commits() {
        let mut value = question();
        value["related_commits"] = json!(["abc1234", "def5678"]);
        let question: KnowledgeQuestion = serde_json::from_value(value).unwrap();

        let feedback = prepare_question(&question, "Question".into());

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
        let location = feedback.location.unwrap();
        assert_eq!(location.file, "src/main.rs");
        assert_eq!(location.line, Some(5));
    }

    #[test]
    fn questions_keep_location_and_commit_with_a_different_related_commit() {
        let mut value = question();
        value["related_commits"] = json!(["def5678"]);
        let question: KnowledgeQuestion = serde_json::from_value(value).unwrap();

        let feedback = prepare_question(&question, "Question".into());

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
        let location = feedback.location.unwrap();
        assert_eq!(location.file, "src/main.rs");
        assert_eq!(location.line, Some(5));
    }

    #[test]
    fn questions_keep_location_and_commit_without_related_commits() {
        let mut value = question();
        value["related_commits"] = json!([]);
        let question: KnowledgeQuestion = serde_json::from_value(value).unwrap();

        let feedback = prepare_question(&question, "Question".into());

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
        let location = feedback.location.unwrap();
        assert_eq!(location.file, "src/main.rs");
        assert_eq!(location.line, Some(5));
    }

    #[test]
    fn unlocated_questions_have_no_location_commit() {
        let mut question: KnowledgeQuestion = serde_json::from_value(question()).unwrap();
        question.location = None;
        let feedback = prepare_question(&question, "Question".into());
        assert!(feedback.commit.is_none());
        assert!(feedback.location.is_none());
    }

    #[test]
    fn gitlab_unlocated_question_without_related_commits_has_no_target() {
        let mut question = question();
        question["location"] = Value::Null;
        question["related_commits"] = json!([]);

        let feedback = gitlab_feedback("questions", question);

        assert_eq!(feedback.commit, None);
    }

    #[test]
    fn gitlab_unlocated_question_with_one_related_commit_targets_that_commit() {
        let mut question = question();
        question["location"] = Value::Null;
        question["related_commits"] = json!(["abc1234"]);

        let feedback = gitlab_feedback("questions", question);

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
    }

    #[test]
    fn gitlab_unlocated_question_with_duplicate_related_commits_targets_that_commit() {
        let mut question = question();
        question["location"] = Value::Null;
        question["related_commits"] = json!(["abc1234", "abc1234"]);

        let feedback = gitlab_feedback("questions", question);

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
    }

    #[test]
    fn gitlab_unlocated_question_with_distinct_related_commits_has_no_target() {
        let mut question = question();
        question["location"] = Value::Null;
        question["related_commits"] = json!(["abc1234", "def5678"]);

        let feedback = gitlab_feedback("questions", question);

        assert_eq!(feedback.commit, None);
    }

    #[test]
    fn gitlab_recommendation_without_related_commits_has_no_target() {
        let mut recommendation = recommendation();
        recommendation["related_commits"] = json!([]);

        let feedback = gitlab_feedback("recommendations", recommendation);

        assert_eq!(feedback.commit, None);
    }

    #[test]
    fn gitlab_recommendation_with_one_related_commit_targets_that_commit() {
        let mut recommendation = recommendation();
        recommendation["related_commits"] = json!(["abc1234"]);

        let feedback = gitlab_feedback("recommendations", recommendation);

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
    }

    #[test]
    fn gitlab_recommendation_with_duplicate_related_commits_targets_that_commit() {
        let mut recommendation = recommendation();
        recommendation["related_commits"] = json!(["abc1234", "abc1234"]);

        let feedback = gitlab_feedback("recommendations", recommendation);

        assert_eq!(feedback.commit.as_ref().unwrap().as_ref(), "abc1234");
    }

    #[test]
    fn gitlab_recommendation_with_distinct_related_commits_has_no_target() {
        let mut recommendation = recommendation();
        recommendation["related_commits"] = json!(["abc1234", "def5678"]);

        let feedback = gitlab_feedback("recommendations", recommendation);

        assert_eq!(feedback.commit, None);
    }

    #[test]
    fn gitlab_keeps_commit_targets_with_their_feedback_kinds() {
        let mut question = question();
        question["location"] = Value::Null;
        let mut recommendation = recommendation();
        recommendation["related_commits"] = json!(["def5678"]);
        let mut finding = finding();
        finding["commit"] = json!("fedcba9");
        let document = serde_json::from_value(json!({
            "ordered_commits": ["abc1234", "def5678", "fedcba9"],
            "stages": [],
            "questions": [question],
            "recommendations": [recommendation],
            "findings": [finding],
        }))
        .unwrap();
        let prepared = PreparedReview::for_gitlab(
            &document,
            &crate::gitlab::Repository::parse("group/project").unwrap(),
        );
        let mut targets = prepared
            .items
            .iter()
            .map(|item| {
                let kind = match item.kind {
                    FeedbackKind::Question => "question",
                    FeedbackKind::Recommendation => "recommendation",
                    FeedbackKind::Finding => "finding",
                };
                (kind, item.commit.as_ref().map(AsRef::as_ref))
            })
            .collect::<Vec<_>>();
        targets.sort_unstable();

        assert_eq!(
            targets,
            [
                ("finding", Some("fedcba9")),
                ("question", Some("abc1234")),
                ("recommendation", Some("def5678")),
            ]
        );
    }

    #[test]
    fn gitlab_prefers_explicit_question_location_over_related_commits() {
        let mut question = question();
        question["related_commits"] = json!(["def5678"]);
        let document = serde_json::from_value(json!({
            "ordered_commits": ["abc1234", "def5678"],
            "stages": [],
            "questions": [question],
        }))
        .unwrap();
        let prepared = PreparedReview::for_gitlab(
            &document,
            &crate::gitlab::Repository::parse("group/project").unwrap(),
        );

        assert_eq!(
            prepared.items[0].commit.as_ref().unwrap().as_ref(),
            "abc1234"
        );
        assert_eq!(prepared.items[0].location.as_ref().unwrap().line, Some(5));
    }

    #[test]
    fn gitlab_summary_contains_metadata_usage_and_stages_without_feedback() {
        let document = serde_json::from_value(json!({
            "summary": {"peer_version": "0.16.2"},
            "ordered_commits": ["abc1234"],
            "stages": [{
                "stage": "quality",
                "target": "abc1234",
                "outcome": {
                    "status": "issues",
                    "summary": "Stage summary",
                    "iterations": 2,
                    "usage": [{
                        "provider": "provider",
                        "model": "model",
                        "input_tokens": 100,
                        "output_tokens": 20,
                        "cache_read_tokens": 0,
                        "cache_write_tokens": 0,
                        "cost_usd": 0.001
                    }]
                }
            }],
            "questions": [question()],
            "recommendations": [recommendation()],
            "findings": [finding()],
        }))
        .unwrap();
        let prepared = PreparedReview::for_gitlab(
            &document,
            &crate::gitlab::Repository::parse("group/project").unwrap(),
        );

        let summary = prepared.aggregate(&[], true);

        assert!(summary.contains("## Review summary"));
        assert!(summary.contains("**Peer version:** 0\\.16\\.2"));
        assert!(summary.contains("### Total token usage"));
        assert!(summary.contains("100 input tokens, 20 output tokens"));
        assert!(summary.contains("Stage summary"));
        assert!(summary.contains("**Iterations:** 2"));
        assert!(!summary.contains("## Review questions"));
        assert!(!summary.contains("## Structural recommendations"));
        assert!(!summary.contains("## Review findings"));
        assert_eq!(
            fingerprints(&summary),
            HashSet::from([prepared.summary_fingerprint.unwrap()])
        );
    }

    #[test]
    fn finding_fingerprint_distinguishes_message() {
        let mut value = finding();
        let original = identity("findings", value.clone(), "owner/repo");
        value["message"] = json!("Other issue");

        assert_ne!(identity("findings", value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_severity() {
        let mut value = finding();
        let original = identity("findings", value.clone(), "owner/repo");
        value["severity"] = json!("low");

        assert_ne!(identity("findings", value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_file() {
        let mut value = finding();
        let original = identity("findings", value.clone(), "owner/repo");
        value["file"] = json!("src/other.rs");

        assert_ne!(identity("findings", value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_line() {
        let mut value = finding();
        let original = identity("findings", value.clone(), "owner/repo");
        value["line"] = json!(6);

        assert_ne!(identity("findings", value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_security() {
        let mut value = finding();
        let original = identity("findings", value.clone(), "owner/repo");
        value["security"] = json!({
            "attacker_control": "request",
            "sensitive_operation": "write",
            "impact": "data loss"
        });

        assert_ne!(identity("findings", value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_ignores_repository() {
        let value = json!({
            "commit": "abc1234",
            "severity": "high",
            "message": "Issue"
        });
        assert_eq!(
            identity("findings", value.clone(), "owner/repo"),
            identity("findings", value, "other/repository")
        );
    }

    #[test]
    fn question_fingerprint_ignores_related_commits() {
        let mut value = question();
        let original = identity("questions", value.clone(), "owner/repo");
        value["related_commits"] = json!(["def5678"]);

        assert_eq!(identity("questions", value, "owner/repo"), original);
    }

    #[test]
    fn question_fingerprint_ignores_location_commit() {
        let mut value = question();
        let original = identity("questions", value.clone(), "owner/repo");
        value["location"]["commit"] = json!("def5678");

        assert_eq!(identity("questions", value, "owner/repo"), original);
    }

    #[test]
    fn recommendation_fingerprint_ignores_related_commits() {
        let mut value = json!({
            "kind": "split_commit",
            "message": "Split this",
            "rationale": "Separate concerns",
            "related_commits": ["abc1234"]
        });
        let original = identity("recommendations", value.clone(), "owner/repo");
        value["related_commits"] = json!(["def5678"]);

        assert_eq!(identity("recommendations", value, "owner/repo"), original);
    }

    #[test]
    fn complete_versioned_marker_is_recognized() {
        let hash = "a".repeat(64);
        assert_eq!(fingerprints(&marker(&hash)), HashSet::from([hash]));
    }

    #[test]
    fn duplicate_markers_are_recognized_once() {
        let hash = "a".repeat(64);
        let valid = marker(&hash);
        assert_eq!(
            fingerprints(&format!("{valid}\n{valid}")),
            HashSet::from([hash])
        );
    }

    #[test]
    fn unsupported_marker_version_is_not_recognized() {
        let valid = marker(&"a".repeat(64));
        assert!(fingerprints(&valid.replace(":v1:", ":v2:")).is_empty());
    }

    #[test]
    fn marker_with_uppercase_hash_is_not_recognized() {
        let valid = marker(&"a".repeat(64));
        assert!(fingerprints(&valid.replace('a', "A")).is_empty());
    }

    #[test]
    fn quoted_marker_is_not_recognized() {
        let valid = marker(&"a".repeat(64));
        assert!(fingerprints(&format!("> {valid}")).is_empty());
    }

    #[test]
    fn marker_with_short_hash_is_not_recognized() {
        assert!(fingerprints("<!-- peer-review:v1:abc -->").is_empty());
    }
}
