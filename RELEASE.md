# Neolingua Center : release checklist

First install (marketing site) and in-app updates use **two different channels**.

| Canal | Destination | Artefacts |
|-------|-------------|-----------|
| Site / première install | R2 `download.neolingua.app` | macOS `.dmg`, Windows NSIS `.exe` |
| Updater in-app | GitHub Releases + `latest.json` | `.app.tar.gz` (+ `.sig`), NSIS `.exe` (+ `.sig`) |

Details: [UPDATER.md](UPDATER.md).

## Bump version (lockstep)

Update the same SemVer string in:

- `package.json`
- `package-lock.json` (root package entries)
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

Example for a beta: `1.0.0-beta.1` (SemVer pre-release with a hyphen).

## Test

```bash
npm test
```

## Signed build

Tauri produces **host-OS installers only**:

- on macOS: `.app` + `.dmg` (not NSIS)
- on Windows: NSIS `.exe` (not DMG)

A full release (macOS + Windows) therefore goes through
`.github/workflows/release-center.yml` (matrix `macos` + `windows-latest`), not a
single local `tauri build`. Local Mac builds are for smoke / `/Applications` only.

Windows NSIS is **not** Authenticode-signed: SmartScreen may warn
« unknown publisher ». macOS stays Developer ID + notarized. Only
`TAURI_SIGNING_PRIVATE_KEY` is required for the updater (minisign) on both OS.

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat .secrets/neolingua-center.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npm run tauri build
```

`tauri.conf.json` lists `app`, `dmg`, `nsis`; each runner only emits what its OS
supports. Updater tarballs use `createUpdaterArtifacts`.

Local install (optional, macOS):

```bash
rm -rf "/Applications/Neolingua Center.app"
cp -R "src-tauri/target/release/bundle/macos/Neolingua Center.app" "/Applications/"
```

## Publish first-install binaries to R2

Installers do not embed the transcription model. Expected DMG / NSIS size is
around 50 MB. The model is a separate immutable object:

`https://download.neolingua.app/whisper/ggml-small.bin`

CI uploads installers and the Whisper model. For a local publish of the model,
create a Cloudflare R2 API token (Object Read & Write on `neolingua-downloads`)
and either export:

```bash
export CLOUDFLARE_ACCOUNT_ID="…"
export R2_ACCESS_KEY_ID="…"
export R2_SECRET_ACCESS_KEY="…"
```

or copy `r2.env.example` to `.secrets/r2.env` (gitignored).

```bash
npm run publish-whisper-model
```

Smoke:

```bash
curl -sI https://download.neolingua.app/macos/neolingua-latest.dmg | head
curl -sI https://download.neolingua.app/windows/neolingua-latest.exe | head
curl -sI https://download.neolingua.app/whisper/ggml-small.bin | head
```

Content-Length must be a real installer size (not a ~35 byte stub). The model
must be 487601967 bytes.

## GitHub release (updater)

1. Commit and push
2. Tag `X.Y.Z` (or `1.0.0-beta.1`) matching the bumped version - no `v` prefix
3. Workflow `.github/workflows/release-center.yml` builds platforms and uploads
   `latest.json` via `tauri-action`
4. The same workflow uploads `neolingua-latest.dmg` / `neolingua-latest.exe` to R2
   and publishes `whisper/ggml-small.bin` if it is not already on the bucket

GitHub Actions secrets: `TAURI_SIGNING_PRIVATE_KEY`,
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, Apple signing (`APPLE_*`,
`KEYCHAIN_PASSWORD`), `CLOUDFLARE_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`,
`R2_SECRET_ACCESS_KEY`. Values never belong in git.
