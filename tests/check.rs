use std::process::Command;

use serde_json::json;

#[test]
fn config_discovery_failure_is_printed_as_error_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let working_directory = tmp.path().canonicalize().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_peer"))
        .args(["check", "size", "HEAD"])
        .current_dir(&working_directory)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        json!({
            "status": "error",
            "error": {
                "code": "config_invalid",
                "message": format!(
                    "no .peer/config.toml found from {}",
                    working_directory.display()
                ),
                "is_retryable": false
            }
        })
    );
}
