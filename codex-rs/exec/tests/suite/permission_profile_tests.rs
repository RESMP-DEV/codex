use core_test_support::responses;
use core_test_support::test_codex_exec::test_codex_exec;
use predicates::prelude::*;

#[test]
fn exec_permission_profile_reaches_config_selection() {
    let test = test_codex_exec();

    test.cmd()
        .arg("--skip-git-repo-check")
        .arg("--permission-profile")
        .arg(":unknown")
        .arg("test profile selection")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "default_permissions refers to unknown built-in profile `:unknown`",
        ));
}

#[cfg(not(target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_permission_profile_enforces_named_read_only_sandbox() -> anyhow::Result<()> {
    core_test_support::skip_if_no_network!(Ok(()));

    let test = test_codex_exec();
    std::fs::write(
        test.home_path().join("config.toml"),
        r#"
approval_policy = "never"

[permissions.alphaheng-test]
extends = ":read-only"
"#,
    )?;
    let marker = test.cwd_path().join("profile-write-should-fail.txt");

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("response_1"),
                responses::ev_shell_command_call(
                    "write_marker",
                    "printf blocked > profile-write-should-fail.txt",
                ),
                responses::ev_completed("response_1"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("response_2"),
                responses::ev_assistant_message("response_2", "done"),
                responses::ev_completed("response_2"),
            ]),
        ],
    )
    .await;

    let output = test
        .cmd_with_server(&server)
        .arg("--skip-git-repo-check")
        .arg("--permission-profile")
        .arg("alphaheng-test")
        .arg("verify selected profile")
        .output()?;

    assert!(output.status.success(), "exec run failed: {output:?}");
    assert!(!marker.exists(), "named read-only profile allowed a write");
    let tool_output = response_mock
        .function_call_output_text("write_marker")
        .expect("write_marker output should be model-visible");
    let exit_code = tool_output
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("Exit code: "))
        .and_then(|code| code.trim().parse::<i32>().ok())
        .expect("write_marker output should report an exit code");
    let tool_output_lower = tool_output.to_lowercase();
    assert_ne!(
        exit_code, 0,
        "read-only sandbox should reject write_marker: {tool_output}"
    );
    assert!(
        tool_output_lower.contains("permission denied")
            || tool_output_lower.contains("operation not permitted")
            || tool_output_lower.contains("read-only file system"),
        "write_marker did not report a sandbox denial: {tool_output}"
    );
    assert!(
        String::from_utf8(output.stderr)?.contains("sandbox: read-only"),
        "stderr did not report the selected read-only profile"
    );
    Ok(())
}
