use std::collections::{BTreeSet, HashSet};

use serde_json::{Value, json};

use crate::render::{RenderInput, github};

use super::Repository;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Question,
    Recommendation,
    Finding,
}

pub struct Feedback {
    pub fingerprint: String,
    pub body: String,
    kind: FeedbackKind,
}

pub struct PreparedReview {
    pub items: Vec<Feedback>,
    pub summary_fingerprint: Option<String>,
    parts: Option<github::DocumentParts>,
}

impl PreparedReview {
    pub fn new(input: &RenderInput, repository: &Repository) -> Self {
        let (inputs, parts) = match input {
            RenderInput::Document(document) => (
                document
                    .questions
                    .iter()
                    .cloned()
                    .map(RenderInput::from)
                    .chain(
                        document
                            .recommendations
                            .iter()
                            .cloned()
                            .map(RenderInput::from),
                    )
                    .chain(document.findings.iter().cloned().map(RenderInput::from))
                    .collect::<Vec<_>>(),
                Some(github::DocumentParts::new(
                    document,
                    &repository.to_string(),
                )),
            ),
            item => (vec![item.clone()], None),
        };
        let items: Vec<_> = inputs
            .iter()
            .map(|item| prepare_item(item, repository))
            .collect();
        let summary_fingerprint = match (input, &parts) {
            (RenderInput::Document(document), Some(parts))
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

fn prepare_item(input: &RenderInput, repository: &Repository) -> Feedback {
    let (kind, tag, identity) = match input {
        RenderInput::KnowledgeQuestion(question) => (
            FeedbackKind::Question,
            "question",
            json!({
                "category": question.category,
                "question": question.question,
                "evidence": question.evidence,
                "why_it_matters": question.why_it_matters,
                "location": question.location.as_ref().map(|location| &location.file),
            }),
        ),
        RenderInput::StructuralRecommendation(recommendation) => (
            FeedbackKind::Recommendation,
            "recommendation",
            json!({
                "kind": recommendation.kind,
                "message": recommendation.message,
                "rationale": recommendation.rationale,
            }),
        ),
        RenderInput::Finding(finding) => (
            FeedbackKind::Finding,
            "finding",
            json!({
                "severity": finding.severity,
                "message": finding.message,
                "location": finding.location,
                "security": finding.security,
            }),
        ),
        RenderInput::Document(_) => unreachable!("documents are flattened before preparing items"),
    };
    Feedback {
        fingerprint: fingerprint(tag, identity),
        body: github::render(input, &repository.to_string()),
        kind,
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

    fn identity(value: Value, repo: &str) -> String {
        PreparedReview::new(
            &serde_json::from_value(value).unwrap(),
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

    #[test]
    fn finding_fingerprint_ignores_commit() {
        let mut value = finding();
        let original = identity(value.clone(), "owner/repo");
        value["commit"] = json!("def5678");

        assert_eq!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_message() {
        let mut value = finding();
        let original = identity(value.clone(), "owner/repo");
        value["message"] = json!("Other issue");

        assert_ne!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_severity() {
        let mut value = finding();
        let original = identity(value.clone(), "owner/repo");
        value["severity"] = json!("low");

        assert_ne!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_file() {
        let mut value = finding();
        let original = identity(value.clone(), "owner/repo");
        value["file"] = json!("src/other.rs");

        assert_ne!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_line() {
        let mut value = finding();
        let original = identity(value.clone(), "owner/repo");
        value["line"] = json!(6);

        assert_ne!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_distinguishes_security() {
        let mut value = finding();
        let original = identity(value.clone(), "owner/repo");
        value["security"] = json!({
            "attacker_control": "request",
            "sensitive_operation": "write",
            "impact": "data loss"
        });

        assert_ne!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn finding_fingerprint_ignores_repository() {
        let value = json!({
            "commit": "abc1234",
            "severity": "high",
            "message": "Issue"
        });
        assert_eq!(
            identity(value.clone(), "owner/repo"),
            identity(value, "other/repository")
        );
    }

    #[test]
    fn question_fingerprint_ignores_related_commits() {
        let mut value = question();
        let original = identity(value.clone(), "owner/repo");
        value["related_commits"] = json!(["def5678"]);

        assert_eq!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn question_fingerprint_ignores_location_commit() {
        let mut value = question();
        let original = identity(value.clone(), "owner/repo");
        value["location"]["commit"] = json!("def5678");

        assert_eq!(identity(value, "owner/repo"), original);
    }

    #[test]
    fn recommendation_fingerprint_ignores_related_commits() {
        let mut value = json!({
            "kind": "split_commit",
            "message": "Split this",
            "rationale": "Separate concerns",
            "related_commits": ["abc1234"]
        });
        let original = identity(value.clone(), "owner/repo");
        value["related_commits"] = json!(["def5678"]);

        assert_eq!(identity(value, "owner/repo"), original);
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
