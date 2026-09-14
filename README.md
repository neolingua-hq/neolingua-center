# Neolingua Center

Hôte familial : bibliothèque locale, serveur LAN embarqué, lecture sur TV et
navigateurs du réseau. Gratuit, [AGPL-3.0](LICENSE).

Téléchargements : https://download.neolingua.app

Le nom et le logo Neolingua restent une marque. Voir [TRADEMARKS.md](TRADEMARKS.md).

## Prérequis

- Node.js 20+
- Rust (`rustup`)
- macOS ou Windows

`ffmpeg`, `ffprobe` et `whisper-cli` sont embarqués. Au premier
`tauri dev` / `tauri build`, `npm run fetch-binaries` les télécharge dans
`src-tauri/binaries/` (gitignorés). Le modèle de transcription
(`ggml-small.bin`, ~465 Mo) n’est **pas** dans l’installeur : il est
téléchargé une fois dans les données de l’app, au premier épisode qui a
vraiment besoin d’une transcription.

## Développement

```bash
npm install
npm run tauri dev
```

Au premier lancement, l’assistant de configuration s’ouvre. Les choix sont
persistés dans SQLite :

- macOS : `~/Library/Application Support/fr.neolingua.center/neolingua.sqlite`
- Windows : `%APPDATA%\fr.neolingua.center\neolingua.sqlite`

## Scripts

| Commande | Description |
|----------|-------------|
| `npm run fetch-binaries` | Télécharge ffmpeg/ffprobe/whisper-cli (pas le modèle) |
| `npm run publish-whisper-model` | Publie `ggml-small.bin` sur R2 s’il n’y est pas déjà |
| `npm run dev` | Vite seul (port 1422) |
| `npm run tauri dev` | App desktop + hot reload |
| `npm run tauri build` | Build release (DMG sur macOS, NSIS `.exe` sur Windows) |
| `npm test` | Tests unitaires (Vitest + `cargo test`) |
| `npm run test:unit` | Vitest seul |
| `npm run test:rust` | Tests Rust (`src-tauri`) |
| `npm run quality` | tsc, Clippy, tests |

Outils Clippy (une fois) :

```bash
rustup component add clippy
```

## Licence et source

Copyright (C) 2026 Martin Catty. GNU Affero GPL v3 uniquement. Le code
source correspondant à chaque release est le tag Git du même numéro de
version : https://github.com/neolingua-hq/neolingua-center

Voir [CONTRIBUTING.md](CONTRIBUTING.md) et [SECURITY.md](SECURITY.md).
