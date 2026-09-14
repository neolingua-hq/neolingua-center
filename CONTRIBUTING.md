# Contribuer

Les PR vont vers `develop`. Une branche `feat/…`, `fix/…` ou `chore/…` par
sujet.

## Dev

```bash
npm install
npm run tauri dev
```

Au premier lancement, `ffmpeg`, `ffprobe` et `whisper-cli` sont téléchargés
dans `src-tauri/binaries/` (gitignorés). Ne les commite pas. Ne commite pas
non plus `ggml-small.bin` ni quoi que ce soit sous `.secrets/`.

## Tests

```bash
npm test
npm run quality
```

`quality` lance `tsc`, Clippy et les tests.

## Secrets

Pas de clés, tokens, `.p12`, `.p8` ou `.env` dans git. Les noms de variables
(`R2_SECRET_ACCESS_KEY`, etc.) dans les docs et le workflow, oui. Les valeurs,
non.
