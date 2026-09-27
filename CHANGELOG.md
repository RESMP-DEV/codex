# Changelog

## Unreleased

- Added a typed `codex exec --permission-profile/-P` selector so execution
  layers can activate a dynamically supplied named permission profile without
  encoding the selection as a raw config override. The selector rejects legacy
  sandbox, automatic-review, and unsandboxed permission modes before config
  construction, including selectors inherited from the root `codex` command.
- Fix missing command lifecycle events when unified exec cannot create a process, preserving approval rejection and sandbox retry behavior. Thanks @Marvinthebored ([#44557](https://github.com/openai/codex/issues/44557)).

Upstream Codex release history is available on the
[releases page](https://github.com/openai/codex/releases).
