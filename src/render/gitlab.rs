use super::RenderDocument;

pub use super::hosted::DocumentParts;

#[cfg_attr(not(test), expect(dead_code))]
pub fn render(document: &RenderDocument, repo: &str) -> String {
    DocumentParts::for_gitlab(document, repo).render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> RenderDocument {
        serde_json::from_value(json!({
            "ordered_commits": ["abc1234"],
            "findings": [{
                "commit": "abc1234",
                "severity": "high",
                "message": "Inspect this change.",
                "file": "src/a b#한].rs",
                "line": 7
            }],
            "stages": [{
                "stage": "quality",
                "target": "abc1234",
                "outcome": {
                    "status": "failed",
                    "failure": {"type": "execution", "reason": "Review incomplete."}
                }
            }]
        }))
        .unwrap()
    }

    fn document_with_stage_summary(summary: &str) -> RenderDocument {
        let mut document = document();
        document.stages[0].outcome = crate::render::RenderStageOutcome::Clean {
            summary: summary.into(),
            iterations: 1,
            usage: Default::default(),
        };
        document
    }

    #[test]
    fn renders_nested_project_and_encoded_file_links_with_alerts_and_details() {
        let output = render(&document(), "group/subgroup/project");

        assert!(output.contains("https://gitlab.com/group/subgroup/project/-/commit/abc1234"));
        assert!(output.contains(
            "https://gitlab.com/group/subgroup/project/-/blob/abc1234/src/a%20b%23%ED%95%9C%5D.rs#L7"
        ));
        assert!(output.contains("<details>\n<summary>Stage: quality"));
        assert!(output.contains("<summary><strong>References:</strong></summary>"));
        assert!(output.contains("> [!WARNING]\n> Review incomplete\\."));
        assert!(!output.contains("github.com"));
    }

    #[test]
    fn neutralizes_all_mentions_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "@all".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — @\u{200b}all\n"));
    }

    #[test]
    fn neutralizes_issue_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See #12".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See \\#\u{200b}12\n"));
    }

    #[test]
    fn neutralizes_merge_request_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See !34".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See \\!\u{200b}34\n"));
    }

    #[test]
    fn neutralizes_cross_project_merge_request_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See group/project!5".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See group/project\\!\u{200b}5\n"));
    }

    #[test]
    fn neutralizes_snippet_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See $42".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See $\u{200b}42\n"));
    }

    #[test]
    fn neutralizes_milestone_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See %milestone".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See %\u{200b}milestone\n"));
    }

    #[test]
    fn neutralizes_label_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See ~label".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See \\~\u{200b}label\n"));
    }

    #[test]
    fn neutralizes_epic_references_in_finding_messages() {
        let mut document = document();
        document.findings[0].message = "See &9".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n- **finding/high** — See &\u{200b}9\n"));
    }

    #[test]
    fn neutralizes_group_mentions_in_markdown_stage_headings() {
        let mut document = document();
        document.stages[0].stage = "@group/subgroup".into();
        let output = render(&document, "group/project");

        assert!(output.contains("\n## Stage: @\u{200b}group/subgroup\n"));
    }

    #[test]
    fn neutralizes_group_mentions_in_html_stage_summaries() {
        let mut document = document();
        document.stages[0].stage = "@group/subgroup".into();
        let output = render(&document, "group/project");

        assert!(output.contains(
            "\n<summary>Stage: @\u{200b}group/subgroup - Status: failed - Target: abc1234</summary>\n"
        ));
    }

    #[test]
    fn escapes_html_tags_in_stage_names() {
        let mut document = document();
        document.stages[0].stage = "quality </summary>".into();
        let output = render(&document, "group/project");

        assert!(output.contains(
            "\n<summary>Stage: quality &lt;/summary&gt; - Status: failed - Target: abc1234</summary>\n"
        ));
    }

    #[test]
    fn neutralizes_quick_actions_at_the_start_of_stage_summaries() {
        let document = document_with_stage_summary("/close");
        let output = render(&document, "group/project");

        assert!(output.contains("\n\n/\u{200b}close\n\n"));
    }

    #[test]
    fn neutralizes_quick_actions_after_leading_spaces_in_stage_summaries() {
        let document = document_with_stage_summary("  /close");
        let output = render(&document, "group/project");

        assert!(output.contains("\n\n  /\u{200b}close\n\n"));
    }

    #[test]
    fn neutralizes_quick_actions_after_a_leading_tab_in_stage_summaries() {
        let document = document_with_stage_summary("\t/close");
        let output = render(&document, "group/project");

        assert!(output.contains("\n\n\t/\u{200b}close\n\n"));
    }

    #[test]
    fn neutralizes_mentions_in_stage_summaries() {
        let document = document_with_stage_summary("Notify @maintainer");
        let output = render(&document, "group/project");

        assert!(output.contains("\n\nNotify @\u{200b}maintainer\n\n"));
    }

    #[test]
    fn currency_values_do_not_become_snippet_references() {
        let mut document = document();
        document.context_usage = Some(
            serde_json::from_value(json!([{
                "provider": "provider",
                "model": "model",
                "input_tokens": 10,
                "output_tokens": 2,
                "cache_read_tokens": 0,
                "cache_write_tokens": 0,
                "cost_usd": 1.25
            }]))
            .unwrap(),
        );
        let output = render(&document, "group/project");
        assert!(output.contains("**Cost:** $\u{200b}1.250000"));
        assert!(!output.contains("$1"));
    }

    #[test]
    fn untrusted_peer_version_cannot_inject_lines_or_mentions() {
        let mut document = document();
        document.summary = Some(crate::review::ReviewSummary {
            peer_version: "custom\n/close\n@all".into(),
        });
        let output = render(&document, "group/project");
        assert!(output.contains("**Peer version:** custom /close @\u{200b}all"));
        assert!(!output.lines().any(|line| line.trim() == "/close"));
    }
}
