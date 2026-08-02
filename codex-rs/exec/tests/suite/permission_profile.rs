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
