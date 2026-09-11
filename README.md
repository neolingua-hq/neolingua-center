# Neolingua Center

Hôte familial Neolingua : configuration locale, serveur embarqué (étapes suivantes), visionnage sur TV / navigateurs du LAN.

## Prérequis

- Node.js 20+
- Rust (rustup)
- macOS ou Windows (cibles V1)

Les outils de conversion (`ffmpeg` / `ffprobe`) et la transcription (`whisper-cli` +
modèle `ggml-small`) sont **embarqués** dans l'app.
Au premier `tauri dev` / `tauri build`, `npm run fetch-binaries` les télécharge
dans `src-tauri/binaries/` et `src-tauri/resources/whisper/` (ignorés par git).
Le modèle Whisper pèse ~465 Mo.

## Développement

```bash
npm install
npm run tauri dev
```

Au premier lancement, l'assistant de configuration s'ouvre. Les choix sont persistés dans SQLite :

- macOS : `~/Library/Application Support/fr.neolingua.center/neolingua.sqlite`
- Windows : `%APPDATA%\fr.neolingua.center\neolingua.sqlite`

## Étape 1 (actuelle)

- Squelette Tauri 2 (cockpit, pas de lecteur vidéo)
- SQLite + migrations (`app_settings`)
- Wizard : bienvenue, dossiers médias (plusieurs), options (base), réseau (placeholder), terminé
- Logo : même PNG que le web (`poc/client/public/neolingua-logo.png`)

## Scripts

| Commande | Description |
|----------|-------------|
| `npm run fetch-binaries` | Télécharge ffmpeg/ffprobe/whisper-cli + modèle ggml-small |
| `npm run dev` | Vite seul (port 1422) |
| `npm run tauri dev` | App desktop + hot reload |
| `npm run tauri build` | Build release (DMG sur macOS, NSIS `.exe` sur Windows) |
| `npm test` | Tests unitaires (Vitest + `cargo test`) |
| `npm run test:unit` | Vitest seul |
| `npm run test:rust` | Tests Rust (`src-tauri`) |
| `npm run quality` | Audit qualité (tsc, clippy, outdated, audit, tests, …) |

Outils Rust optionnels pour `npm run quality` (une fois) :

```bash
cargo install cargo-outdated cargo-audit --locked
```
