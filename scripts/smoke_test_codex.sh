#!/usr/bin/env bash
# Smoke test the freshly-published codex snapshot with a real model round-trip.
#
# After a daily rebuild, send a trivial prompt through `codex exec` and assert
# the literal reply comes back. On failure, flip the `current` package symlink
# back to the `previous-good` snapshot (a complete package, so the app-server
# daemon keeps working — unlike a bare binary rollback), re-pin the daemon, and
# fire a loud macOS notification so a human (or another model) can intervene.
# Exits 0 on pass, 1 on smoke failure, 2 on configuration errors (e.g., no
# rollback target).
#
# Usage:
#   scripts/smoke_test_codex.sh                 # probe + rollback-on-fail + notify
#   scripts/smoke_test_codex.sh --no-rollback   # probe only, leave snapshot selected
#   scripts/smoke_test_codex.sh --no-notify     # skip the macOS modal/banner
#   scripts/smoke_test_codex.sh --probe '...'   # override the probe prompt
#
# Design: see docs/superpowers/specs/ (smoke-test design doc) and
# ~/AlphaHENG/docs/operations/codex_local_package_flow.md.

set -euo pipefail

# --- configuration -----------------------------------------------------------

INSTALL_PATH="${CODEX_INSTALL_PATH:-${HOME}/.local/bin/codex}"
LIB_ROOT="${CODEX_LIB_ROOT:-${HOME}/.local/lib/alphaheng}"
LOG_DIR="${CODEX_SMOKE_LOG_DIR:-${HOME}/.codex/smoke-logs}"
PROBE_PROMPT_DEFAULT="Reply with exactly: codex-smoke-ok"
PROBE_REPLY_TOKEN="codex-smoke-ok"
PROBE_TIMEOUT_SECONDS="${CODEX_SMOKE_TIMEOUT:-60}"

probe_prompt="$PROBE_PROMPT_DEFAULT"
do_rollback=1
do_notify=1

usage() {
  cat <<'EOF'
Usage: scripts/smoke_test_codex.sh [options]

Options:
  --no-rollback         Leave the freshly-published snapshot selected on failure
                        (default: flip `current` back to previous-good).
  --no-notify           Skip the macOS blocking modal + banner on failure.
  --probe <prompt>      Override the probe prompt (default replies with a
                        fixed token that the assertion looks for).
  --install-path <path> Codex install path (default: ~/.local/bin/codex).
  -h, --help            Show this help.

Environment:
  CODEX_INSTALL_PATH   Override the codex install path.
  CODEX_LIB_ROOT       Snapshot root (default: ~/.local/lib/alphaheng); expects
                      `current` and `previous-good` symlinks managed by
                      scripts/build_and_link_codex.sh.
  CODEX_SMOKE_LOG_DIR  Directory for per-run logs (default: ~/.codex/smoke-logs).
  CODEX_SMOKE_TIMEOUT  Per-probe timeout in seconds (default: 60).
EOF
}

while (($# > 0)); do
  case "$1" in
    --no-rollback) do_rollback=0; shift ;;
    --no-notify) do_notify=0; shift ;;
    --probe)
      if (($# < 2)); then echo "error: --probe requires a value" >&2; exit 2; fi
      probe_prompt="$2"; shift 2 ;;
    --install-path)
      if (($# < 2)); then echo "error: --install-path requires a value" >&2; exit 2; fi
      INSTALL_PATH="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

current_link="${LIB_ROOT}/current"
previous_good="${LIB_ROOT}/previous-good"

mkdir -p "$LOG_DIR"
run_stamp="$(date +%Y%m%d%H%M%S)"
log_path="${LOG_DIR}/smoke-${run_stamp}.log"

log() { printf '[smoke %s] %s\n' "$(date +%H:%M:%S)" "$*" | tee -a "$log_path" >&2; }

# --- preflight ---------------------------------------------------------------

if [[ ! -e "$INSTALL_PATH" ]]; then
  echo "error: codex not found at $INSTALL_PATH — nothing to smoke test" >&2
  exit 2
fi

# Resolve the real file behind the symlink chain, if any. If INSTALL_PATH is
# already a regular file, -e above passed and readlink returns the path
# unchanged.
real_binary="$(readlink -f "$INSTALL_PATH" 2>/dev/null || echo "$INSTALL_PATH")"
if [[ ! -f "$real_binary" ]]; then
  echo "error: resolved binary $real_binary is not a regular file" >&2
  exit 2
fi

release_sha="$(shasum -a 256 "$real_binary" | awk '{print $1}')"
log "install path:  $INSTALL_PATH"
log "real binary:   $real_binary"
log "release sha:   $release_sha"
log "probe prompt:  $probe_prompt"

# --- resolve the rollback target BEFORE the probe ------------------------------
#
# The rollback target is the previous-good snapshot directory published by
# scripts/build_and_link_codex.sh — a complete package layout, so the
# app-server daemon can serve from it after the flip (a bare binary cannot).

rollback_target=""
if ((do_rollback)); then
  if [[ -L "$previous_good" ]]; then
    candidate="$(basename "$(readlink "$previous_good")")"
    if [[ -n "$candidate" && -x "${LIB_ROOT}/packages/${candidate}/bin/codex" ]]; then
      rollback_target="$candidate"
    fi
  fi
  if [[ -n "$rollback_target" ]]; then
    log "rollback target: packages/${rollback_target}"
  else
    log "WARNING: no previous-good snapshot — rollback will not be possible"
  fi
fi

# --- run the probe -----------------------------------------------------------

log "running codex exec probe (timeout ${PROBE_TIMEOUT_SECONDS}s)..."

probe_output_path="${LOG_DIR}/probe-${run_stamp}.out"
probe_rc=0
# shellcheck disable=SC2086
timeout "$PROBE_TIMEOUT_SECONDS" \
  "$INSTALL_PATH" exec \
    --dangerously-bypass-approvals-and-sandbox \
    --dangerously-bypass-hook-trust \
    --skip-git-repo-check \
    "$probe_prompt" \
  </dev/null >"$probe_output_path" 2>&1 || probe_rc=$?

probe_output="$(cat "$probe_output_path")"

if [[ "$probe_rc" -ne 0 ]]; then
  log "probe FAILED (exit $probe_rc)"
elif grep -qF "$PROBE_REPLY_TOKEN" "$probe_output_path"; then
  log "probe PASSED (reply contains '$PROBE_REPLY_TOKEN')"
  log "outcome: SUCCESS — snapshot is good and stays selected"
  exit 0
else
  log "probe FAILED (exit 0 but reply missing '$PROBE_REPLY_TOKEN')"
  probe_rc=99
fi

# --- failure path: rollback + notify ----------------------------------------

log "probe output (first 40 lines):"
sed -n '1,40p' "$probe_output_path" | tee -a "$log_path" >&2
log "full probe output saved: $probe_output_path"

if [[ "$do_rollback" -eq 1 ]]; then
  if [[ -n "$rollback_target" ]]; then
    if ln -sfn "packages/${rollback_target}" "$current_link"; then
      log "ROLLBACK: flipped current -> packages/${rollback_target}"
      # The daemon keeps its own copy under CODEX_HOME; re-pin it to the
      # rolled-back snapshot. Best-effort: a failure here leaves the CLI rolled
      # back but the daemon stale, which the hint below covers.
      if timeout 90 "$INSTALL_PATH" app-server daemon update --from-cli --yes >>"$log_path" 2>&1; then
        log "daemon re-pinned to the rolled-back snapshot"
      else
        log "WARNING: daemon re-pin failed — run: codex app-server daemon update --from-cli --yes"
      fi
    else
      log "ROLLBACK FAILED: could not flip $current_link (manual recovery required)"
    fi
  else
    log "ROLLBACK IMPOSSIBLE: no previous-good snapshot under ${LIB_ROOT}/packages"
    log "codex is currently broken — manual intervention required"
  fi
else
  log "--no-rollback in effect: leaving freshly-published snapshot selected"
fi

if [[ "$do_notify" -eq 1 ]]; then
  # Extract a compact error excerpt for the dialog body.
  excerpt="$(head -c 400 "$probe_output_path" | tr '\n' ' ' | sed 's/"/'\''/g')"
  if command -v osascript >/dev/null 2>&1; then
    # Notification Center banner first (non-blocking, for when you're away).
    osascript -e "display notification \"codex smoke test failed — see modal\" with title \"‼️ codex smoke test FAILED\" sound name \"Basso\"" >/dev/null 2>&1 || true
    # Terminal bell.
    printf '\a' >&2 || true
    # Blocking modal — literally in your face, requires a click.
    osascript <<APPLESCRIPT >/dev/null 2>&1 || true
display dialog "codex smoke test FAILED after daily rebuild.

Probe: $probe_prompt
Outcome: exit $probe_rc, reply did not contain '$PROBE_REPLY_TOKEN'.

$([[ "$do_rollback" -eq 1 ]] && echo "Rolled back to previous-good snapshot: ${rollback_target:-none available}" || echo "No rollback (--no-rollback in effect).")

Excerpt: $excerpt

Manual intervention required. Log: $log_path" with title "‼️ codex smoke test FAILED" buttons {"Acknowledge"} default button 1 with icon stop
APPLESCRIPT
  else
    log "osascript not available — cannot pop macOS notification"
  fi
fi

log "outcome: FAILURE — exit 1"
exit 1
