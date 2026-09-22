#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build the local `codex` release and publish it as a versioned package snapshot.

The app-server daemon only starts from a complete package layout
(codex-package.json, bin/, codex-path/, codex-resources/), so each build is
assembled into an immutable snapshot directory and selected via a `current`
symlink. Rollback = flip `current` back (scripts/smoke_test_codex.sh does this
automatically on smoke failure).

Layout:
  <lib-root>/packages/<yyyymmdd-HHMM>-<gitsha>[-dirty]/   snapshot package dirs
  <lib-root>/current                        -> packages/<newest snapshot>
  <lib-root>/previous-good                  -> packages/<prior build>
  ~/.local/bin/codex                        -> <lib-root>/current/bin/codex

Usage:
  scripts/build_and_link_codex.sh [options]

Options:
  --no-build                 Skip cargo release build and only publish/verify.
  --install-path <path>      Install path to update (default: ~/.local/bin/codex).
  --lib-root <path>          Snapshot root (default: ~/.local/lib/alphaheng).
  --keep-snapshots <count>   Non-protected snapshots to retain (default: 4).
  -h, --help                 Show this help text.

Environment:
  CODEX_LIB_ROOT            Default for --lib-root.
  CODEX_KEEP_SNAPSHOTS      Default for --keep-snapshots.
EOF
}

do_build=1
install_path="${HOME}/.local/bin/codex"
lib_root="${CODEX_LIB_ROOT:-${HOME}/.local/lib/alphaheng}"
keep_snapshots="${CODEX_KEEP_SNAPSHOTS:-4}"

while (($# > 0)); do
  case "$1" in
    --no-build)
      do_build=0
      shift
      ;;
    --install-path)
      if (($# < 2)); then
        echo "error: --install-path requires a value" >&2
        exit 1
      fi
      install_path="$2"
      shift 2
      ;;
    --lib-root)
      if (($# < 2)); then
        echo "error: --lib-root requires a value" >&2
        exit 1
      fi
      lib_root="$2"
      shift 2
      ;;
    --keep-snapshots)
      if (($# < 2)); then
        echo "error: --keep-snapshots requires a value" >&2
        exit 1
      fi
      keep_snapshots="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
codex_rs_dir="${repo_root}/codex-rs"
release_bin="${codex_rs_dir}/target/release/codex"
packages_root="${lib_root}/packages"

if ((do_build)); then
  echo "Building optimized codex binary..."
  (
    cd "${codex_rs_dir}"
    cargo build -p codex-cli --release
  )
fi

if [[ ! -x "${release_bin}" ]]; then
  echo "error: release binary missing or not executable: ${release_bin}" >&2
  exit 1
fi

mkdir -p "$(dirname "${install_path}")" "${packages_root}"

# --- assemble the snapshot package ---------------------------------------------
git_sha="$(git -C "${repo_root}" rev-parse --short=8 HEAD 2>/dev/null || echo nogit)"
if [[ -n "$(git -C "${repo_root}" status --porcelain 2>/dev/null | head -1)" ]]; then
  git_sha="${git_sha}-dirty"
fi
snapshot_name="$(date +%Y%m%d-%H%M)-${git_sha}"
snapshot_dir="${packages_root}/${snapshot_name}"

package_args=(
  --package-dir "${snapshot_dir}"
  --entrypoint-bin "${release_bin}"
  --force
)
host_path="$(dirname "${install_path}")/codex-code-mode-host"
if [[ -x "${host_path}" ]]; then
  package_args+=(--code-mode-host-bin "${host_path}")
fi
if command -v rg >/dev/null 2>&1; then
  package_args+=(--rg-bin "$(command -v rg)")
fi
if [[ -x /bin/zsh ]]; then
  package_args+=(--zsh-bin /bin/zsh)
fi
echo "Assembling snapshot package ${snapshot_name}..."
(
  cd "${repo_root}"
  CODEX_REPO_ROOT="${repo_root}" python3 scripts/build_codex_package.py "${package_args[@]}"
)

package_bin="${snapshot_dir}/bin/codex"
if [[ ! -x "${package_bin}" ]]; then
  echo "error: packaged binary missing or not executable: ${package_bin}" >&2
  exit 1
fi

release_sha="$(shasum -a 256 "${release_bin}" | awk '{print $1}')"
package_sha="$(shasum -a 256 "${package_bin}" | awk '{print $1}')"
if [[ "${release_sha}" != "${package_sha}" ]]; then
  echo "error: packaged binary checksum does not match release binary" >&2
  exit 1
fi

# --- select the snapshot ---------------------------------------------------------
# Record the outgoing selection as previous-good (the smoke test's rollback
# target), then flip `current`. Relative link targets keep the tree movable.
previous_target=""
if [[ -L "${lib_root}/current" ]]; then
  previous_target="$(basename "$(readlink "${lib_root}/current")")"
fi
ln -sfn "packages/${snapshot_name}" "${lib_root}/current"
if [[ -n "${previous_target}" \
  && "${previous_target}" != "${snapshot_name}" \
  && -d "${packages_root}/${previous_target}" ]]; then
  ln -sfn "packages/${previous_target}" "${lib_root}/previous-good"
fi

ln -sfn "${lib_root}/current/bin/codex" "${install_path}"

install_sha="$(shasum -a 256 "${install_path}" | awk '{print $1}')"
if [[ "${release_sha}" != "${install_sha}" ]]; then
  echo "error: linked binary checksum does not match release binary" >&2
  exit 1
fi

echo "Install path: ${install_path}"
echo "Linked to: $(readlink "${install_path}")"
echo "Snapshot: ${snapshot_name}"

# --- prune old snapshots -----------------------------------------------------------
# Keep the newest `keep_snapshots` non-protected snapshots; the targets of
# `current` and `previous-good` are always retained (rollback safety).
kept=0
while IFS= read -r snap; do
  [[ -n "${snap}" ]] || continue
  if [[ "${snap}" == "${snapshot_name}" || "${snap}" == "${previous_target}" ]]; then
    continue
  fi
  ((kept++))
  if ((kept > keep_snapshots)); then
    rm -rf "${packages_root:?}/${snap}"
    echo "Pruned old snapshot: ${snap}"
  fi
done < <(ls -1 "${packages_root}" | sort -r)

# --- code-mode host skew check ------------------------------------------------
# cargo build -p codex-cli does not compile codex-code-mode-host (it needs the
# V8 ptrcomp_sandbox artifacts), so a daily rebuild silently leaves a stale host
# in place. A host older than the codex binary breaks the TUI tool bridge with
# "failed to decode code-mode IPC frame: missing field ..." errors.
if [[ ! -x "${host_path}" ]]; then
  echo "warning: no codex-code-mode-host beside the install path; the code-mode tool bridge will be unavailable." >&2
elif [[ "${host_path}" -ot "${release_bin}" ]]; then
  echo "warning: ${host_path} predates the freshly built codex binary." >&2
  echo "warning: a stale host breaks the tool bridge via IPC frame decode errors." >&2
  echo "warning: rebuild it (cargo build -p codex-code-mode-host --release, V8 artifacts via scripts/codex_package/v8.py)" >&2
  echo "warning: or install the official host from the matching openai/codex release (codex-code-mode-host-aarch64-apple-darwin.tar.gz)." >&2
fi
echo "Release SHA: ${release_sha}"

# --- daemon refresh hint --------------------------------------------------------
# The running daemon keeps its own copy under CODEX_HOME; point the user at the
# update command instead of restarting it here (it may interrupt active work).
if [[ -e "${HOME}/.codex/packages/app-server-daemon/current" ]]; then
  echo "note: a daemon package is installed; run 'codex app-server daemon update --from-cli --yes' to switch it to this snapshot." >&2
fi

if command -v codex >/dev/null 2>&1; then
  command_path="$(command -v codex)"
  command_sha="$(shasum -a 256 "${command_path}" | awk '{print $1}')"
  echo "command -v codex: ${command_path}"
  if [[ "${command_sha}" == "${release_sha}" ]]; then
    echo "Resolved command matches the new release binary."
  else
    echo "warning: resolved command does not match the new release binary." >&2
    echo "warning: adjust PATH if you want this install path to take precedence." >&2
  fi
  codex --version
fi
