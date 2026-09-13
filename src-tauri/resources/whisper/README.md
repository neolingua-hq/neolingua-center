# Whisper model (app data)

`ggml-small.bin` is not shipped in the installer. Center downloads it once into
app data on the first transcription that needs it:

- macOS: `~/Library/Application Support/fr.neolingua.center/whisper/`
- Windows: `%APPDATA%\fr.neolingua.center\whisper\`

Canonical URL: `https://download.neolingua.app/whisper/ggml-small.bin`

A leftover `ggml-small.bin` in this folder is used only by `tauri dev` and must
not be committed.
