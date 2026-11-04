#!/usr/bin/env bash
# Re-sign a Windows NSIS installer for the Tauri updater AFTER Authenticode signing.
# Authenticode changes file bytes, so the previous minisign .sig / latest.json entry
# for that artifact is invalid and must be regenerated.
#
# Usage (CI):
#   export TAURI_SIGNING_PRIVATE_KEY=...
#   export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=...
#   ./scripts/resign-windows-updater.sh path/to/signed-setup.exe
#
# Writes path/to/signed-setup.exe.sig next to the exe.

set -euo pipefail

exe="${1:-}"
if [[ -z "$exe" || ! -f "$exe" ]]; then
  echo "usage: $0 <signed-nsis.exe>" >&2
  exit 1
fi

if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  echo "TAURI_SIGNING_PRIVATE_KEY is required" >&2
  exit 1
fi

: "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:=}"

npx --yes @tauri-apps/cli@2 signer sign -f "$exe"
sig="${exe}.sig"
if [[ ! -f "$sig" ]]; then
  echo "expected signature at $sig" >&2
  exit 1
fi

echo "Wrote $sig"
