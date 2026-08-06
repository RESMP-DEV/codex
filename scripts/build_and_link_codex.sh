#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build and relink the local `codex` command to this repo's release binary.

Usage:
  scripts/build_and_link_codex.sh [options]

Options:
  --no-build             Skip cargo release build and only relink/verify.
  --backup-existing      Move existing install target to a timestamped backup before relinking.
  --install-path <path>  Install path to update (default: ~/.local/bin/codex).
  -h, --help             Show this help text.

Examples:
  scripts/build_and_link_codex.sh
  scripts/build_and_link_codex.sh --no-build
  scripts/build_and_link_codex.sh --backup-existing
EOF
}

do_build=1
backup_existing=0
install_path="${HOME}/.local/bin/codex"

while (($# > 0)); do
  case "$1" in
    --no-build)
      do_build=0
      shift
      ;;
    --backup-existing)
      backup_existing=1
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

mkdir -p "$(dirname -- "${install_path}")"

if [[ -e "${install_path}" || -L "${install_path}" ]]; then
  if ((backup_existing)); then
    backup_path="${install_path}.prelink.$(date +%Y%m%d%H%M%S)"
    mv "${install_path}" "${backup_path}"
    echo "Backed up existing binary to ${backup_path}"
  fi
fi

ln -sfn "${release_bin}" "${install_path}"

release_sha="$(shasum -a 256 "${release_bin}" | awk '{print $1}')"
install_sha="$(shasum -a 256 "${install_path}" | awk '{print $1}')"

if [[ "${release_sha}" != "${install_sha}" ]]; then
  echo "error: linked binary checksum does not match release binary" >&2
  exit 1
fi

echo "Install path: ${install_path}"
echo "Linked to: $(readlink "${install_path}")"
echo "Release SHA: ${release_sha}"

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
