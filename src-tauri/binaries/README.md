# Sidecar binaries (ffmpeg / ffprobe / whisper-cli)

Self-contained tools bundled via Tauri `externalBin`. Not committed to git.

```bash
npm run fetch-binaries
```

Expected files (host triple suffix):

- `ffmpeg-aarch64-apple-darwin`
- `ffprobe-aarch64-apple-darwin`
- `whisper-cli-aarch64-apple-darwin`

Also downloads the Whisper model (not a sidecar binary):

- `../resources/whisper/ggml-small.bin` (~465 MB)

Sources:

- ffmpeg/ffprobe: [eugeneware/ffmpeg-static](https://github.com/eugeneware/ffmpeg-static/releases) (`b6.1.1`)
- whisper-cli: built from [ggml-org/whisper.cpp](https://github.com/ggml-org/whisper.cpp) `master` (Metal on macOS, no OpenMP / no Homebrew libs)
- model: Hugging Face `ggerganov/whisper.cpp` → `ggml-small.bin`
