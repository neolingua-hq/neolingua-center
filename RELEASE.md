# Neolingua Center — release checklist

First install (marketing site) and in-app updates use **two different channels**.

| Canal | Destination | Artefacts |
|-------|-------------|-----------|
| Site / première install | R2 `download.neolingua.app` | macOS `.dmg`, Windows NSIS `.exe` |
| Updater in-app | GitHub Releases + `latest.json` | `.app.tar.gz` (+ `.sig`), NSIS `.exe` (+ `.sig`) |

Details: [DOWNLOADS.md](../website/DOWNLOADS.md), [UPDATER.md](UPDATER.md).

## 1. Bump version (lockstep)

Update the same SemVer string in:

- `package.json`
- `package-lock.json` (root package entries)
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

Example for a beta: `1.0.0-beta.1` (SemVer pre-release with a hyphen).

## 2. Test

```bash
cd neolingua-center
npm test
```

## 3. Signed build

Tauri produces **host-OS installers only**:

- on macOS: `.app` + `.dmg` (not NSIS)
- on Windows: NSIS `.exe` (not DMG)

A full release (macOS + Windows) therefore goes through
`.github/workflows/release-center.yml` (matrix `macos` + `windows-latest`), not a
single local `tauri build`. Local Mac builds are for smoke / `/Applications` only.

Windows NSIS is **not** Authenticode-signed (intentional): SmartScreen may warn
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

## 4. Publish first-install binaries to R2

The DMG is larger than wrangler's ~300 MiB CLI limit, so uploads use the **R2 S3
API** (multipart via `aws s3 cp`).

Create an R2 API token once (Object Read & Write on `neolingua-downloads`):

https://dash.cloudflare.com/?to=/:account/r2/api-tokens

Preferred source: AWS Secrets Manager `neolingua/r2/neolingua-release`
(managed by `homelab/infrastructure/cloudflare/r2`). Token name: `neolingua-release`.

Then either export:

```bash
export CLOUDFLARE_ACCOUNT_ID="…"   # from wrangler whoami / dashboard
export R2_ACCESS_KEY_ID="…"
export R2_SECRET_ACCESS_KEY="…"
```

or write `neolingua-center/.secrets/r2.env` (gitignored; see `r2.env.example`):

```bash
CLOUDFLARE_ACCOUNT_ID=…
R2_ACCESS_KEY_ID=…
R2_SECRET_ACCESS_KEY=…
```

Publish (script local, non versionné : `scripts/publish-downloads.mjs`) :

```bash
npm run publish-downloads
```

Writes:

- `macos/neolingua-latest.dmg` + versioned copy
- `windows/neolingua-latest.exe` + versioned copy (if an NSIS build is present)

Smoke:

```bash
curl -sI https://download.neolingua.app/macos/neolingua-latest.dmg | head
curl -sI https://download.neolingua.app/windows/neolingua-latest.exe | head
```

Content-Length must be a real installer size (not a ~35 byte stub).

## 5. GitHub release (updater)

When the GitHub repo and secrets are ready:

1. Commit and push
2. Tag `X.Y.Z` (or `1.0.0-beta.1`) matching the bumped version - no `v` prefix
3. Workflow `.github/workflows/release-center.yml` builds platforms and uploads
   `latest.json` via `tauri-action`
4. The same workflow uploads `neolingua-latest.dmg` / `neolingua-latest.exe` to R2

Secrets: `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`,
`CLOUDFLARE_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`.

## 6. Site HTML

Stable download URLs do not change. Redeploy Pages only if the marketing page
copy or link paths changed:

```bash
cd website && ./deploy.sh
```
