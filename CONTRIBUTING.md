# Contributing

PRs target `develop`. One `feat/…`, `fix/…`, or `chore/…` branch per topic.

Need Node.js 22.22+ (CI uses 24), Rust (`rustup`), and macOS or Windows.
See [README.md](README.md) for the full prerequisites.

## Dev

```bash
npm install
npm run tauri dev
```

On first run, `ffmpeg`, `ffprobe`, and `whisper-cli` are downloaded into
`src-tauri/binaries/` (gitignored). Do not commit them. Do not commit
`ggml-small.bin` either, or anything under `.secrets/`.

`npm install` installs the git pre-commit hook (husky). It runs rustfmt +
Clippy on staged `.rs` files, and `tsc` + oxlint on the Vite / TypeScript
front end (and viewer JS, excluding `*.min.js`). Clippy needs the sidecars:
run `npm run fetch-binaries` once.

## Tests

```bash
npm test
npm run lint
npm run quality
```

`lint`: tsc, oxlint, rustfmt `--check`, Clippy (`-D warnings`).
`quality`: lint + tests.

## Secrets

No keys, tokens, `.p12`, `.p8`, or `.env` files in git. Variable names
(`R2_SECRET_ACCESS_KEY`, and so on) in docs and the workflow are fine. Values
are not.
