use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::json;

fn run_render_with_args(args: &[&str], input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_peer"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn run_render(format: &str, input: &str) -> std::process::Output {
    run_render_with_args(&["render", "--format", format], input)
}

fn run_render_verbose(format: &str, input: &str) -> std::process::Output {
    run_render_with_args(&["--verbose", "render", "--format", format], input)
}

fn success_envelope() -> serde_json::Value {
    json!({
        "status": "success",
        "data": {
            "check": "size",
            "target": "abc1234",
            "summary": "The commit is appropriately sized.",
            "findings": [],
            "confidence": 0.9,
            "iterations": 1,
            "is_exhausted": false,
            "exhaustion_reason": null,
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cost_usd": 0.001,
                "model": "test-model"
            }
        }
    })
}

#[test]
fn renders_terminal_output_from_stdin() {
    let output = run_render("terminal", &success_envelope().to_string());

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "\
Check: size
Target: abc1234
Status: ok

The commit is appropriately sized.

Findings: none

Confidence: 90% | Iterations: 1
"
    );
}

#[test]
fn renders_usage_from_stdin_with_verbose() {
    let output = run_render_verbose("terminal", &success_envelope().to_string());

    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Confidence: 90% | Iterations: 1")
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("[verbose] Usage: 100 input, 20 output, $0.001000 (test-model)")
    );
}

#[test]
fn renders_pretty_json_from_stdin() {
    let input = success_envelope();
    let mut expected = input.clone();
    expected["data"].as_object_mut().unwrap().remove("usage");
    let output = run_render("json", &input.to_string());

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        expected
    );
}

#[test]
fn renders_usage_in_json_from_stdin_with_verbose() {
    let input = success_envelope();
    let mut expected = input.clone();
    expected["data"].as_object_mut().unwrap().remove("usage");
    let output = run_render_verbose("json", &input.to_string());

    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        expected
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("[verbose] Usage: 100 input, 20 output, $0.001000 (test-model)")
    );
}

#[test]
fn renders_markdown_output_from_stdin() {
    let output = run_render("markdown", &success_envelope().to_string());

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.starts_with("## Check: size\n"));
    assert!(output.contains("- **Status:** ok"));
    assert!(output.contains("### Findings\n\nNone."));
    assert!(output.contains("### Metadata"));
}

#[test]
fn renders_invalid_input_error_in_selected_format() {
    let output = run_render("terminal", "{");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("invalid check envelope:")
    );
}
