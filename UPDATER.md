# Neolingua Center : auto-update (GitHub Releases)

## Channels

| Canal | Role | Where |
|-------|------|-------|
| **First install** | Download buttons | Cloudflare R2 (`download.neolingua.app`) - see [RELEASE.md](RELEASE.md) |
| **In-app updater** | Existing installs | GitHub Releases + static `latest.json` (this document) |

Center uses the official Tauri updater plugin. Signed update artifacts are published
to GitHub Releases together with a static `latest.json` manifest.

The Whisper model is **not** part of the updater payload. It lives in app data and
is downloaded once from `download.neolingua.app/whisper/ggml-small.bin`.

Configured endpoint:

`https://github.com/neolingua-hq/neolingua-center/releases/latest/download/latest.json`

If the GitHub owner/repo differs, update `plugins.updater.endpoints` in
`src-tauri/tauri.conf.json` and keep this doc in sync.

## One-time key setup

A keypair was generated for this project. The **public** key is already in
`tauri.conf.json`. The **private** key must never be committed.

Local copy (gitignored, optional convenience):

- `.secrets/neolingua-center.key`
- `.secrets/neolingua-center.key.pub`

If those files are missing, restore them from your password manager or regenerate:

```bash
npm run tauri signer generate -- -w .secrets/neolingua-center.key
```

Then replace `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` with the contents of
`.secrets/neolingua-center.key.pub` (single line / file content as produced by the CLI).
Regenerating the key **breaks updates** for existing installs.

Add GitHub repository secrets:

- `TAURI_SIGNING_PRIVATE_KEY` - full contents of `neolingua-center.key`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` - empty string if the key has no password

## Release flow

See [RELEASE.md](RELEASE.md) for the full checklist (R2 first-install + GitHub updater).

1. Bump version in lockstep:
   - `package.json`
   - `src-tauri/tauri.conf.json`
   - `src-tauri/Cargo.toml`
2. Commit, push, and tag `X.Y.Z` (no `v` prefix; must be greater than the version users already run).
3. Workflow `.github/workflows/release-center.yml` builds:
   - macOS `aarch64` (`.app` + `.dmg` + updater `.tar.gz` + `.sig`)
   - macOS `x86_64`
   - Windows NSIS (setup `.exe` + `.sig`)
4. `tauri-action` merges platforms into `latest.json` on the release.
5. A follow-up job uploads `macos/neolingua-latest.dmg` and `windows/neolingua-latest.exe` to R2.

## Manual local sign (optional)

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat .secrets/neolingua-center.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npm run tauri build
```

## UX in the app

- Background check at boot, then every hour while Center stays open
- Status bar (next to version): orange download icon when outdated; click version or
  icon to check / reopen the update banner
- Banner under the header:
  - Available: **Plus tard** / **Au prochain démarrage** / **Mettre à jour**
  - Scheduled: **Annuler** / **Mettre à jour maintenant** (auto-install on next launch)
- Before relaunch, the LAN HTTP server is stopped (`stop_lan_server`)
- localStorage: `updateLastCheck`, `updateDismissed`, `updateOnNextLaunch`

## First install vs updater

- **macOS** : Developer ID + notarization (Gatekeeper).
- **Windows** : pas de certificat Authenticode pour l’instant. Le NSIS
  CI est unsigned ; SmartScreen peut afficher « éditeur inconnu »
  (Plus d'infos -> Exécuter quand même).

The Tauri updater verifies minisign signatures of update payloads; it does not
replace OS code-signing for first install.

## Test checklist

- Install a release build (macOS `/Applications`, Windows NSIS)
- Publish a higher version tag with artifacts + `latest.json`
- Open Center and confirm the banner, or use Vérifier les mises à jour
- After install, confirm the LAN viewer still starts on the configured port
