#!/usr/bin/env bash
# Typecheck, Clippy, and unit tests. Run from the repo root via `npm run quality`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ ! -d node_modules ]]; then
  echo "node_modules missing; run npm install" >&2
  exit 1
fi

npx --no-install tsc --noEmit
(cd src-tauri && cargo clippy)
npm test
