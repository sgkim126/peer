use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::json;

fn run_render(format: &str, input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_peer"))
        .args(["render", "--format", format])
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
Usage: 100 input, 20 output, $0.001000 (test-model)
"
    );
}

#[test]
fn renders_pretty_json_from_stdin() {
    let envelope = success_envelope();
    let output = run_render("json", &envelope.to_string());

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        envelope
    );
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
