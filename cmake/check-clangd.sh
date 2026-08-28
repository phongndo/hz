#!/usr/bin/env bash
set -euo pipefail

readonly compile_commands="build/debug/compile_commands.json"
if [[ ! -f "${compile_commands}" ]]; then
  printf "error: %s is missing; run \`just configure\` first\n" "${compile_commands}" >&2
  exit 1
fi

for source_file in "$@"; do
  diagnostics="$(
    clangd \
      --check="${source_file}" \
      --check-locations=0 \
      --check-warnings \
      --compile-commands-dir="$(dirname "${compile_commands}")" \
      --enable-config \
      --log=error 2>&1
  )" || {
    status=$?
    printf '%s\n' "${diagnostics}" >&2
    exit "${status}"
  }

  if [[ -n "${diagnostics}" ]]; then
    printf '%s\n' "${diagnostics}" >&2
    exit 1
  fi
done
