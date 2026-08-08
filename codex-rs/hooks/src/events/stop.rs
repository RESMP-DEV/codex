use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::items::HookPromptFragment;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::common;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::engine::output_parser;
use crate::schema::NullableString;
use crate::schema::StopCommandInput;
use crate::schema::SubagentStopCommandInput;

const CHANGED_FILES_GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_CHANGED_FILES: usize = 1_000;

#[derive(Debug, Clone)]
pub struct StopRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub stop_hook_active: bool,
    pub last_assistant_message: Option<String>,
    pub target: StopHookTarget,
}

#[derive(Debug, Clone)]
pub enum StopHookTarget {
    Stop,
    SubagentStop {
        agent_id: String,
        agent_type: String,
        agent_transcript_path: Option<PathBuf>,
    },
}

impl StopHookTarget {
    fn event_name(&self) -> HookEventName {
        match self {
            Self::Stop => HookEventName::Stop,
            Self::SubagentStop { .. } => HookEventName::SubagentStop,
        }
    }

    fn matcher_input(&self) -> Option<&str> {
        match self {
            Self::Stop => None,
            Self::SubagentStop { agent_type, .. } => Some(agent_type.as_str()),
        }
    }
}

#[derive(Debug, Default)]
pub struct StopOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
    pub should_stop: bool,
    pub stop_reason: Option<String>,
    pub should_block: bool,
    pub block_reason: Option<String>,
    pub continuation_fragments: Vec<HookPromptFragment>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct StopHandlerData {
    should_stop: bool,
    stop_reason: Option<String>,
    should_block: bool,
    block_reason: Option<String>,
    continuation_fragments: Vec<HookPromptFragment>,
}

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    request: &StopRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        request.target.event_name(),
        request.target.matcher_input(),
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: StopRequest,
) -> StopOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        request.target.event_name(),
        request.target.matcher_input(),
    );
    if matched.is_empty() {
        return StopOutcome {
            hook_events: Vec::new(),
            should_stop: false,
            stop_reason: None,
            should_block: false,
            block_reason: None,
            continuation_fragments: Vec::new(),
        };
    }

    // Lazily collect changed files only when hooks are actually matched.
    // This avoids the cost of spawning git when no Stop hooks are configured.
    let changed_files = collect_changed_files(request.cwd.as_path()).await;

    let input_json = match request.target {
        StopHookTarget::Stop => {
            let input = StopCommandInput {
                session_id: request.session_id.to_string(),
                turn_id: request.turn_id.clone(),
                transcript_path: NullableString::from_path(request.transcript_path.clone()),
                cwd: request.cwd.display().to_string(),
                hook_event_name: "Stop".to_string(),
                model: request.model.clone(),
                permission_mode: request.permission_mode.clone(),
                stop_hook_active: request.stop_hook_active,
                last_assistant_message: NullableString::from_string(
                    request.last_assistant_message.clone(),
                ),
                changed_files,
            };
            match serde_json::to_string(&input) {
                Ok(input_json) => input_json,
                Err(error) => {
                    return serialization_failure_outcome(
                        common::serialization_failure_hook_events(
                            matched,
                            Some(request.turn_id),
                            format!("failed to serialize stop hook input: {error}"),
                        ),
                    );
                }
            }
        }
        StopHookTarget::SubagentStop {
            agent_id,
            agent_type,
            agent_transcript_path,
        } => {
            let input = SubagentStopCommandInput {
                session_id: request.session_id.to_string(),
                turn_id: request.turn_id.clone(),
                transcript_path: NullableString::from_path(request.transcript_path.clone()),
                agent_transcript_path: NullableString::from_path(agent_transcript_path),
                cwd: request.cwd.display().to_string(),
                hook_event_name: "SubagentStop".to_string(),
                model: request.model.clone(),
                permission_mode: request.permission_mode.clone(),
                stop_hook_active: request.stop_hook_active,
                agent_id,
                agent_type,
                last_assistant_message: NullableString::from_string(
                    request.last_assistant_message.clone(),
                ),
                changed_files,
            };
            match serde_json::to_string(&input) {
                Ok(input_json) => input_json,
                Err(error) => {
                    return serialization_failure_outcome(
                        common::serialization_failure_hook_events(
                            matched,
                            Some(request.turn_id),
                            format!("failed to serialize subagent stop hook input: {error}"),
                        ),
                    );
                }
            }
        }
    };

    let results = dispatcher::execute_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        Some(request.turn_id),
        parse_completed,
    )
    .await;

    let aggregate = aggregate_results(results.iter().map(|result| &result.data));

    StopOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
        should_stop: aggregate.should_stop,
        stop_reason: aggregate.stop_reason,
        should_block: aggregate.should_block,
        block_reason: aggregate.block_reason,
        continuation_fragments: aggregate.continuation_fragments,
    }
}

/// Collect files changed in the working directory relative to HEAD.
///
/// Returns `None` when the directory is not a git repository, git is
/// unavailable, or there are no changes. This is intentionally best-effort:
/// hook consumers should treat `None` as "unknown" rather than "no changes."
async fn collect_changed_files(cwd: &std::path::Path) -> Option<Vec<String>> {
    let (diff_output, untracked_output) = tokio::join!(
        run_changed_files_git_query(cwd, &["diff", "--name-only", "-z", "HEAD"]),
        run_changed_files_git_query(cwd, &["ls-files", "--others", "--exclude-standard", "-z"]),
    );
    let (Some(diff_output), Some(untracked_output)) = (diff_output, untracked_output) else {
        return None;
    };

    let capacity = diff_output.stdout.iter().filter(|byte| **byte == 0).count()
        + untracked_output
            .stdout
            .iter()
            .filter(|byte| **byte == 0)
            .count();
    let mut files: Vec<String> = Vec::with_capacity(capacity);
    let invalid_paths = extend_nul_delimited_paths(&mut files, &diff_output.stdout)
        + extend_nul_delimited_paths(&mut files, &untracked_output.stdout);
    if invalid_paths > 0 {
        tracing::warn!(
            invalid_paths,
            "ignored changed file paths that were not valid UTF-8"
        );
    }

    finalize_changed_files(files)
}

async fn run_changed_files_git_query(
    cwd: &std::path::Path,
    args: &[&str],
) -> Option<std::process::Output> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null());
    command.kill_on_drop(true);

    match tokio::time::timeout(CHANGED_FILES_GIT_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => Some(output),
        Ok(Ok(output)) => {
            tracing::warn!(?args, status = ?output.status, "changed-files git query failed");
            None
        }
        Ok(Err(error)) => {
            tracing::warn!(?args, %error, "failed to execute changed-files git query");
            None
        }
        Err(_) => {
            tracing::warn!(
                ?args,
                timeout_seconds = CHANGED_FILES_GIT_TIMEOUT.as_secs(),
                "changed-files git query timed out"
            );
            None
        }
    }
}

fn finalize_changed_files(files: Vec<String>) -> Option<Vec<String>> {
    let changed_file_count = files.len();
    let mut kept = std::collections::BTreeSet::new();
    let mut truncated = false;
    for file in files {
        kept.insert(file);
        if kept.len() > MAX_CHANGED_FILES {
            kept.pop_last();
            truncated = true;
        }
    }
    if truncated {
        tracing::warn!(
            changed_file_count,
            limit = MAX_CHANGED_FILES,
            "truncated changed files supplied to stop hooks"
        );
    }
    let files = kept.into_iter().collect::<Vec<_>>();

    if files.is_empty() { None } else { Some(files) }
}

fn extend_nul_delimited_paths(files: &mut Vec<String>, output: &[u8]) -> usize {
    let mut invalid_paths = 0;
    for path in output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        match std::str::from_utf8(path) {
            Ok(path) => files.push(path.to_string()),
            Err(_) => invalid_paths += 1,
        }
    }
    invalid_paths
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<StopHandlerData> {
    let mut entries = Vec::new();
    let mut status = HookRunStatus::Completed;
    let mut should_stop = false;
    let mut stop_reason = None;
    let mut should_block = false;
    let mut block_reason = None;
    let mut continuation_prompt = None;
    let hook_event_name = match handler.event_name {
        HookEventName::Stop | HookEventName::SubagentStop => handler.event_name,
        event_name => {
            panic!("expected stop hook event, got {event_name:?}");
        }
    };

    match run_result.error.as_deref() {
        Some(error) => {
            status = HookRunStatus::Failed;
            entries.push(HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error.to_string(),
            });
        }
        None => match run_result.exit_code {
            Some(0) => {
                let trimmed_stdout = run_result.stdout.trim();
                if trimmed_stdout.is_empty() {
                } else if let Some(parsed) = match hook_event_name {
                    HookEventName::Stop => output_parser::parse_stop(&run_result.stdout),
                    HookEventName::SubagentStop => {
                        output_parser::parse_subagent_stop(&run_result.stdout)
                    }
                    _ => unreachable!("validated stop hook event"),
                } {
                    if let Some(system_message) = parsed.universal.system_message {
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Warning,
                            text: system_message,
                        });
                    }
                    let _ = parsed.universal.suppress_output;
                    if !parsed.universal.continue_processing {
                        status = HookRunStatus::Stopped;
                        should_stop = true;
                        stop_reason = parsed.universal.stop_reason.clone();
                        if let Some(stop_reason_text) = parsed.universal.stop_reason {
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Stop,
                                text: stop_reason_text,
                            });
                        }
                    } else if let Some(invalid_block_reason) = parsed.invalid_block_reason {
                        status = HookRunStatus::Failed;
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Error,
                            text: invalid_block_reason,
                        });
                    } else if parsed.should_block {
                        if let Some(reason) =
                            parsed.reason.as_deref().and_then(common::trimmed_non_empty)
                        {
                            status = HookRunStatus::Blocked;
                            should_block = true;
                            block_reason = Some(reason.clone());
                            continuation_prompt = Some(reason.clone());
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Feedback,
                                text: reason,
                            });
                        } else {
                            status = HookRunStatus::Failed;
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Error,
                                text: match hook_event_name {
                                    HookEventName::Stop => "Stop hook returned decision:block without a non-empty reason",
                                    HookEventName::SubagentStop => "SubagentStop hook returned decision:block without a non-empty reason",
                                    _ => unreachable!("validated stop hook event"),
                                }
                                .to_string(),
                            });
                        }
                    }
                } else {
                    status = HookRunStatus::Failed;
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Error,
                        text: match hook_event_name {
                            HookEventName::Stop => "hook returned invalid stop hook JSON output",
                            HookEventName::SubagentStop => {
                                "hook returned invalid subagent stop hook JSON output"
                            }
                            _ => unreachable!("validated stop hook event"),
                        }
                        .to_string(),
                    });
                }
            }
            Some(2) => {
                if let Some(reason) = common::trimmed_non_empty(&run_result.stderr) {
                    status = HookRunStatus::Blocked;
                    should_block = true;
                    block_reason = Some(reason.clone());
                    continuation_prompt = Some(reason.clone());
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Feedback,
                        text: reason,
                    });
                } else {
                    status = HookRunStatus::Failed;
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Error,
                        text: match hook_event_name {
                            HookEventName::Stop => {
                                "Stop hook exited with code 2 but did not write a continuation prompt to stderr"
                            }
                            HookEventName::SubagentStop => {
                                "SubagentStop hook exited with code 2 but did not write a continuation prompt to stderr"
                            }
                            _ => unreachable!("validated stop hook event"),
                        }
                        .to_string(),
                    });
                }
            }
            Some(exit_code) => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: format!("hook exited with code {exit_code}"),
                });
            }
            None => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: "hook exited without a status code".to_string(),
                });
            }
        },
    }

    let completed = HookCompletedEvent {
        turn_id,
        run: dispatcher::completed_summary(handler, &run_result, status, entries),
    };
    let continuation_fragments = continuation_prompt
        .map(|prompt| {
            vec![HookPromptFragment::from_single_hook(
                prompt,
                completed.run.id.clone(),
            )]
        })
        .unwrap_or_default();

    dispatcher::ParsedHandler {
        completed,
        data: StopHandlerData {
            should_stop,
            stop_reason,
            should_block,
            block_reason,
            continuation_fragments,
        },
        completion_order: 0,
    }
}

fn aggregate_results<'a>(
    results: impl IntoIterator<Item = &'a StopHandlerData>,
) -> StopHandlerData {
    let results = results.into_iter().collect::<Vec<_>>();
    let should_stop = results.iter().any(|result| result.should_stop);
    let stop_reason = results.iter().find_map(|result| result.stop_reason.clone());
    let should_block = !should_stop && results.iter().any(|result| result.should_block);
    let block_reason = if should_block {
        common::join_text_chunks(
            results
                .iter()
                .filter_map(|result| result.block_reason.clone())
                .collect(),
        )
    } else {
        None
    };
    let continuation_fragments = if should_block {
        results
            .iter()
            .filter(|result| result.should_block)
            .flat_map(|result| result.continuation_fragments.clone())
            .collect()
    } else {
        Vec::new()
    };

    StopHandlerData {
        should_stop,
        stop_reason,
        should_block,
        block_reason,
        continuation_fragments,
    }
}

fn serialization_failure_outcome(hook_events: Vec<HookCompletedEvent>) -> StopOutcome {
    StopOutcome {
        hook_events,
        should_stop: false,
        stop_reason: None,
        should_block: false,
        block_reason: None,
        continuation_fragments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use codex_protocol::protocol::HookEventName;
    use codex_protocol::protocol::HookOutputEntry;
    use codex_protocol::protocol::HookOutputEntryKind;
    use codex_protocol::protocol::HookRunStatus;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use codex_protocol::items::HookPromptFragment;

    use super::StopHandlerData;
    use super::aggregate_results;
    use super::collect_changed_files;
    use super::extend_nul_delimited_paths;
    use super::finalize_changed_files;
    use super::parse_completed;
    use crate::engine::ConfiguredHandler;
    use crate::engine::command_runner::CommandRunResult;

    #[test]
    fn nul_delimited_paths_preserve_embedded_newlines() {
        let mut files = Vec::new();

        let invalid_paths = extend_nul_delimited_paths(&mut files, b"line\nbreak.rs\0normal.rs\0");

        assert_eq!(invalid_paths, 0);
        assert_eq!(
            files,
            vec!["line\nbreak.rs".to_string(), "normal.rs".to_string()]
        );
    }

    #[test]
    fn nul_delimited_paths_skip_invalid_utf8() {
        let mut files = Vec::new();

        let invalid_paths = extend_nul_delimited_paths(&mut files, b"valid.rs\0invalid-\xff.rs\0");

        assert_eq!(invalid_paths, 1);
        assert_eq!(files, vec!["valid.rs".to_string()]);
    }

    #[test]
    fn changed_files_are_sorted_deduplicated_and_bounded() {
        let files = (0..=super::MAX_CHANGED_FILES)
            .rev()
            .map(|index| format!("file-{index:04}.rs"))
            .chain(std::iter::once("file-0000.rs".to_string()))
            .collect();

        let files = finalize_changed_files(files).expect("non-empty file list");

        assert_eq!(files.len(), super::MAX_CHANGED_FILES);
        assert_eq!(files.first().map(String::as_str), Some("file-0000.rs"));
        assert_eq!(files.last().map(String::as_str), Some("file-0999.rs"));
    }

    #[tokio::test]
    async fn changed_files_include_tracked_and_untracked_paths() {
        let repo = tempdir().expect("create temp repo");
        run_git(repo.path(), &["init", "-b", "main"]);
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.path().join("tracked.rs"), "original\n").expect("write tracked file");
        run_git(repo.path(), &["add", "tracked.rs"]);
        run_git(
            repo.path(),
            &[
                "-c",
                "user.name=Codex Test",
                "-c",
                "user.email=codex@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "initial",
            ],
        );
        std::fs::write(repo.path().join("tracked.rs"), "changed\n").expect("modify tracked file");
        std::fs::write(repo.path().join("untracked.rs"), "new\n").expect("write untracked file");

        assert_eq!(
            collect_changed_files(repo.path()).await,
            Some(vec!["tracked.rs".to_string(), "untracked.rs".to_string(),])
        );
    }

    #[tokio::test]
    async fn changed_files_are_unknown_before_the_first_commit() {
        let repo = tempdir().expect("create temp repo");
        run_git(repo.path(), &["init", "-b", "main"]);
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.path().join("untracked.rs"), "new\n").expect("write untracked file");

        assert_eq!(collect_changed_files(repo.path()).await, None);
    }

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    #[test]
    fn block_decision_with_reason_sets_continuation_prompt() {
        let parsed = parse_completed(
            &handler(),
            run_result(
                Some(0),
                r#"{"decision":"block","reason":"retry with tests"}"#,
                "",
            ),
            Some("turn-1".to_string()),
        );

        assert_eq!(
            parsed.data,
            StopHandlerData {
                should_stop: false,
                stop_reason: None,
                should_block: true,
                block_reason: Some("retry with tests".to_string()),
                continuation_fragments: vec![HookPromptFragment {
                    text: "retry with tests".to_string(),
                    hook_run_id: parsed.completed.run.id.clone(),
                }],
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Blocked);
    }

    #[test]
    fn block_decision_without_reason_is_invalid() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), r#"{"decision":"block"}"#, ""),
            Some("turn-1".to_string()),
        );

        assert_eq!(parsed.data, StopHandlerData::default());
        assert_eq!(parsed.completed.run.status, HookRunStatus::Failed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "Stop hook returned decision:block without a non-empty reason".to_string(),
            }]
        );
    }

    #[test]
    fn continue_false_overrides_block_decision() {
        let parsed = parse_completed(
            &handler(),
            run_result(
                Some(0),
                r#"{"continue":false,"stopReason":"done","decision":"block","reason":"keep going"}"#,
                "",
            ),
            Some("turn-1".to_string()),
        );

        assert_eq!(
            parsed.data,
            StopHandlerData {
                should_stop: true,
                stop_reason: Some("done".to_string()),
                should_block: false,
                block_reason: None,
                continuation_fragments: Vec::new(),
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Stopped);
    }

    #[test]
    fn exit_code_two_uses_stderr_feedback_only() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(2), "ignored stdout", "retry with tests"),
            Some("turn-1".to_string()),
        );

        assert_eq!(
            parsed.data,
            StopHandlerData {
                should_stop: false,
                stop_reason: None,
                should_block: true,
                block_reason: Some("retry with tests".to_string()),
                continuation_fragments: vec![HookPromptFragment {
                    text: "retry with tests".to_string(),
                    hook_run_id: parsed.completed.run.id.clone(),
                }],
            }
        );
        assert_eq!(parsed.completed.run.status, HookRunStatus::Blocked);
    }

    #[test]
    fn exit_code_two_without_stderr_does_not_block() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(2), "", "   "),
            /*turn_id*/ None,
        );

        assert_eq!(parsed.data, StopHandlerData::default());
        assert_eq!(parsed.completed.run.status, HookRunStatus::Failed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text:
                    "Stop hook exited with code 2 but did not write a continuation prompt to stderr"
                        .to_string(),
            }]
        );
    }

    #[test]
    fn block_decision_with_blank_reason_fails_instead_of_blocking() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), "{\"decision\":\"block\",\"reason\":\"   \"}", ""),
            Some("turn-1".to_string()),
        );

        assert_eq!(parsed.data, StopHandlerData::default());
        assert_eq!(parsed.completed.run.status, HookRunStatus::Failed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "Stop hook returned decision:block without a non-empty reason".to_string(),
            }]
        );
    }

    #[test]
    fn invalid_stdout_fails_instead_of_silently_nooping() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), "not json", ""),
            Some("turn-1".to_string()),
        );

        assert_eq!(parsed.data, StopHandlerData::default());
        assert_eq!(parsed.completed.run.status, HookRunStatus::Failed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "hook returned invalid stop hook JSON output".to_string(),
            }]
        );
    }

    #[test]
    fn aggregate_results_concatenates_blocking_reasons_in_declaration_order() {
        let aggregate = aggregate_results([
            &StopHandlerData {
                should_stop: false,
                stop_reason: None,
                should_block: true,
                block_reason: Some("first".to_string()),
                continuation_fragments: vec![HookPromptFragment::from_single_hook(
                    "first", "run-1",
                )],
            },
            &StopHandlerData {
                should_stop: false,
                stop_reason: None,
                should_block: true,
                block_reason: Some("second".to_string()),
                continuation_fragments: vec![HookPromptFragment::from_single_hook(
                    "second", "run-2",
                )],
            },
        ]);

        assert_eq!(
            aggregate,
            StopHandlerData {
                should_stop: false,
                stop_reason: None,
                should_block: true,
                block_reason: Some("first\n\nsecond".to_string()),
                continuation_fragments: vec![
                    HookPromptFragment::from_single_hook("first", "run-1"),
                    HookPromptFragment::from_single_hook("second", "run-2"),
                ],
            }
        );
    }

    fn handler() -> ConfiguredHandler {
        ConfiguredHandler {
            event_name: HookEventName::Stop,
            matcher: None,
            command: "echo hook".to_string(),
            timeout_sec: 600,
            status_message: None,
            additional_context_limit: Default::default(),
            source_path: test_path_buf("/tmp/hooks.json").abs(),
            source: codex_protocol::protocol::HookSource::User,
            display_order: 0,
            env: std::collections::HashMap::new(),
        }
    }

    fn run_result(exit_code: Option<i32>, stdout: &str, stderr: &str) -> CommandRunResult {
        CommandRunResult {
            started_at: 1,
            completed_at: 2,
            duration_ms: 1,
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            error: None,
        }
    }
}
