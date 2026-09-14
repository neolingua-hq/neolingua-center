#!/usr/bin/env bash
# Typecheck, oxlint, rustfmt, Clippy, and unit tests.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ ! -d node_modules ]]; then
  echo "node_modules missing; run npm install" >&2
  exit 1
fi

npx --no-install tsc --noEmit
npx --no-install oxlint --deny-warnings src vite.config.ts scripts src-tauri/viewer
node scripts/lint-rs.mjs fmt-check
node scripts/lint-rs.mjs clippy
npm test
