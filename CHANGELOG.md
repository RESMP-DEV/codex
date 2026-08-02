# Changelog

## Unreleased

- Added a typed `codex exec --permission-profile/-P` selector so execution
  layers can activate a dynamically supplied named permission profile without
  encoding the selection as a raw config override. The selector rejects legacy
  sandbox, automatic-review, and unsandboxed permission modes at parse time.

Upstream Codex release history is available on the
[releases page](https://github.com/openai/codex/releases).
