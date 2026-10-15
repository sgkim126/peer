use super::*;

use std::assert_matches;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn fake_gh(script: &str) -> (tempfile::TempDir, GhClient) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("gh executable");
    fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    (directory, GhClient::new(executable, Duration::from_secs(5)))
}

fn invocation(args: &[&str], stdin: Option<Vec<u8>>) -> GhInvocation {
    GhInvocation {
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        stdin,
    }
}

#[tokio::test]
async fn passes_arguments_without_shell_interpretation() {
    let (_directory, client) = fake_gh("printf '%s\\n' \"$@\"");

    let output = client
        .run(&invocation(
            &["a b", "$(echo unsafe)", "x; echo unsafe"],
            None,
        ))
        .await
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"a b\n$(echo unsafe)\nx; echo unsafe\n");
}

#[tokio::test]
async fn drains_large_output_while_writing_large_binary_input() {
    let (_directory, client) = fake_gh(
        "dd if=/dev/zero bs=196608 count=1 2>/dev/null\n\
         dd if=/dev/zero bs=196608 count=1 1>&2 2>/dev/null\n\
         cat",
    );
    let stdin = b"first line\nsecond line\n\0\xff".repeat(8192);

    let output = client
        .run(&invocation(&[], Some(stdin.clone())))
        .await
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let mut expected_stdout = vec![0; 196_608];
    expected_stdout.extend(stdin);
    assert_eq!(output.stdout, expected_stdout);
    assert_eq!(output.stderr, vec![0; 196_608]);
}

#[tokio::test]
async fn reports_broken_pipe_when_successful_child_ignores_input() {
    let (_directory, client) = fake_gh("exit 0");

    let error = client
        .run(&invocation(&[], Some(vec![b'x'; 196_608])))
        .await
        .unwrap_err();

    assert_matches!(&error, GhRunError::Io(error) if error.kind() == io::ErrorKind::BrokenPipe);
    assert!(error.may_have_run());
}

#[tokio::test]
async fn preserves_output_when_failed_child_ignores_input() {
    let (_directory, client) = fake_gh("printf response\nprintf 'failure detail' >&2\nexit 17");

    let output = client
        .run(&invocation(&[], Some(vec![b'x'; 196_608])))
        .await
        .unwrap();

    assert_eq!(output.status.code(), Some(17));
    assert_eq!(output.stdout, b"response");
    assert_eq!(output.stderr, b"failure detail");
}

#[tokio::test]
async fn missing_executable_reports_installation_and_path() {
    let directory = tempfile::tempdir().unwrap();
    let client = GhClient::new(directory.path().join("missing"), Duration::from_secs(5));

    let error = client.check_available().await.unwrap_err();

    assert_matches!(&error, GhRunError::Spawn(error) if error.kind() == io::ErrorKind::NotFound);
    assert!(!error.may_have_run());
    assert!(error.to_string().contains("install GitHub CLI"));
    assert!(error.to_string().contains("PATH"));
}

#[tokio::test]
async fn nonexecutable_file_reports_permissions() {
    let (_directory, client) = fake_gh("exit 0");
    fs::set_permissions(&client.executable, fs::Permissions::from_mode(0o644)).unwrap();

    let error = client.check_available().await.unwrap_err();

    assert_matches!(&error, GhRunError::Spawn(error) if error.kind() == io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("permissions"));
}

#[tokio::test]
async fn availability_only_checks_version_and_closes_stdin() {
    let (_directory, client) = fake_gh(
        "[ \"$#\" -eq 1 ] && [ \"$1\" = --version ] || exit 8\n\
         if read -r unexpected; then exit 9; fi\n\
         printf 'gh version 2.101.0\\n'",
    );

    client.check_available().await.unwrap();
}

#[tokio::test]
async fn availability_reports_failed_version_command() {
    let (_directory, client) = fake_gh("printf 'invalid executable\\n' >&2\nexit 12");

    let error = client.check_available().await.unwrap_err();

    assert_matches!(
        &error,
        GhRunError::NonZeroExit { status: Some(12), stderr } if stderr == "invalid executable"
    );
    assert!(
        error
            .to_string()
            .contains("gh command failed with status 12")
    );
}

#[tokio::test]
async fn timeout_kills_and_reaps_child() {
    let (directory, client) = fake_gh("printf '%s' \"$$\" > \"$1\"\nexec /bin/sleep 60");
    let pid_file = directory.path().join("pid");

    let error = client
        .run(&invocation(&[pid_file.to_str().unwrap()], None))
        .await
        .unwrap_err();

    assert_matches!(error, GhRunError::Timeout);
    assert!(error.may_have_run());
    let pid = fs::read_to_string(pid_file).unwrap();
    assert!(!process_exists(&pid));
}

#[tokio::test]
async fn cancellation_kills_child() {
    let (directory, client) = fake_gh("printf '%s' \"$$\" > \"$1\"\nexec /bin/sleep 60");
    let pid_file = directory.path().join("pid");
    let invocation = invocation(&[pid_file.to_str().unwrap()], None);
    let task = tokio::spawn(async move { client.run(&invocation).await });
    let pid = wait_for_pid(&pid_file).await;

    task.abort();
    assert_matches!(task.await, Err(error) if error.is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_exists(&pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled gh process must be killed and reaped");
}

async fn wait_for_pid(path: &Path) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = fs::read_to_string(path)
                && !pid.is_empty()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake gh must start")
}

fn process_exists(pid: &str) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn configures_noninteractive_execution_without_authentication_overrides() {
    let client = GhClient::default();
    let invocation = invocation(&[], None);
    let command = client.build_command(&invocation);
    let environment = command.as_std().get_envs().collect::<Vec<_>>();

    for (key, value) in [
        ("GH_PROMPT_DISABLED", Some("1")),
        ("NO_COLOR", Some("1")),
        ("GH_PAGER", Some("cat")),
        ("GH_FORCE_TTY", None),
        ("GH_DEBUG", Some("0")),
    ] {
        assert!(environment.contains(&(OsStr::new(key), value.map(OsStr::new))));
    }
    assert!(
        environment.iter().all(|(key, _)| key != &"GH_TOKEN"
            && key != &"GITHUB_TOKEN"
            && key != &"GH_CONFIG_DIR")
    );
}

#[test]
fn invocation_debug_does_not_expose_stdin() {
    let body = b"private comment body";
    let invocation = invocation(&["pr", "comment"], Some(body.to_vec()));

    let debug = format!("{invocation:?}");

    assert!(!debug.contains("private comment body"));
    assert!(!debug.contains(&format!("{body:?}")));
}
