use std::assert_matches;
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn typescript_tests_pass() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/pi/tests");
    let mut tests = fs::read_dir(&directory)
        .expect("Pi TypeScript test directory must be readable")
        .map(|entry| {
            entry
                .expect("Pi TypeScript test entry must be readable")
                .path()
        })
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".test.ts"))
        })
        .collect::<Vec<_>>();
    tests.sort();
    assert_matches!(tests.as_slice(), [_, ..], "no Pi TypeScript tests found");

    let output = Command::new("node")
        .arg("--test")
        .args(&tests)
        .output()
        .expect("Node.js is required to run the Pi TypeScript tests");

    assert_eq!(
        output.status.code(),
        Some(0),
        "Pi TypeScript tests failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
