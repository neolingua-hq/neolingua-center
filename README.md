# Neolingua Center

Home media host: local library, embedded LAN server, playback on TVs and
browsers on your network. Free, [AGPL-3.0](LICENSE).

Product site and downloads: https://center.neolingua.app/

The Neolingua name and logo remain trademarks. See [TRADEMARKS.md](TRADEMARKS.md).

## Prerequisites

- Node.js 22.22+ (CI uses 24)
- Rust (`rustup`)
- macOS or Windows

`ffmpeg`, `ffprobe`, and `whisper-cli` are bundled. On the first
`tauri dev` / `tauri build`, `npm run fetch-binaries` downloads them into
`src-tauri/binaries/` (gitignored). The transcription model
(`ggml-small.bin`, ~465 MB) is **not** in the installer: it is downloaded
once into app data, on the first episode that actually needs transcription.

## Development

```bash
npm install
npm run tauri dev
```

On first launch, the setup wizard opens. Choices are persisted in SQLite:

- macOS: `~/Library/Application Support/fr.neolingua.center/neolingua.sqlite`
- Windows: `%APPDATA%\fr.neolingua.center\neolingua.sqlite`

## Scripts

| Command | Description |
|---------|-------------|
| `npm run fetch-binaries` | Download ffmpeg/ffprobe/whisper-cli (not the model) |
| `npm run publish-whisper-model` | Publish `ggml-small.bin` to R2 if it is not already there |
| `npm run dev` | Vite only (port 1422) |
| `npm run tauri dev` | Desktop app + hot reload |
| `npm run tauri build` | Release build (DMG on macOS, NSIS `.exe` on Windows) |
| `npm test` | Unit tests (Vitest + `cargo test`) |
| `npm run test:unit` | Vitest only |
| `npm run test:rust` | Rust tests (`src-tauri`) |
| `npm run lint` | tsc, oxlint (Vite/TS), rustfmt, Clippy |
| `npm run quality` | lint + tests |

Rust tooling (once):

```bash
rustup component add rustfmt clippy
```

## License and source

Copyright (C) 2026 Martin Catty. GNU Affero GPL v3 only. The corresponding
source for each release is the Git tag with the same version number:
https://github.com/neolingua-hq/neolingua-center

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).
