use super::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SessionSource;
use pretty_assertions::assert_eq;

#[test]
fn raw_model_is_retained_and_metric_model_is_sanitized() {
    let telemetry = SessionTelemetry::new(
        ThreadId::new(),
        "zai,glm-5.3",
        "zai,glm-5.3",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "tty".to_string(),
        SessionSource::Cli,
    )
    .with_model("zai,glm-5.3", "zai,glm-5.3");

    assert_eq!(telemetry.metadata.model, "zai,glm-5.3");
    assert_eq!(telemetry.metric_model, "zai_glm-5.3");
}

#[test]
fn startup_metric_model_is_sanitized_before_with_model() {
    let telemetry = SessionTelemetry::new(
        ThreadId::new(),
        "zai,glm-5.3",
        "zai,glm-5.3",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "tty".to_string(),
        SessionSource::Cli,
    );

    assert_eq!(telemetry.metadata.model, "zai,glm-5.3");
    assert_eq!(telemetry.metric_model, "zai_glm-5.3");
}

#[test]
fn metric_model_follows_each_with_model_transition() {
    let telemetry = SessionTelemetry::new(
        ThreadId::new(),
        "gpt-5.1",
        "gpt-5.1",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "tty".to_string(),
        SessionSource::Cli,
    )
    .with_model("zai,glm-5.3", "zai,glm-5.3")
    .with_model("claude-3.5-sonnet", "claude-3.5-sonnet");

    assert_eq!(telemetry.metadata.model, "claude-3.5-sonnet");
    assert_eq!(telemetry.metric_model, "claude-3.5-sonnet");

    let telemetry = SessionTelemetry::new(
        ThreadId::new(),
        "gpt-5.1",
        "gpt-5.1",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test_originator".to_string(),
        /*log_user_prompts*/ false,
        "tty".to_string(),
        SessionSource::Cli,
    )
    .with_model("zai,glm-5.3", "zai,glm-5.3");

    assert_eq!(telemetry.metadata.model, "zai,glm-5.3");
    assert_eq!(telemetry.metric_model, "zai_glm-5.3");
}
