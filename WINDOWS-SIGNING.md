# Windows Authenticode signing (titane)

Neolingua Center Windows NSIS installers are signed with a Certum Open Source
Code Signing certificate via **SimplySign** (cloud key, not exportable).

## Architecture

1. GitHub-hosted `windows-latest` builds the NSIS `.exe` (and publishes a first
   updater `.sig` via `tauri-action`).
2. Self-hosted runner on **titane** Authenticode-signs that `.exe` with
   `signtool` + SimplySign Desktop.
3. A follow-up GitHub-hosted job regenerates the Tauri updater `.sig` and
   patches `latest.json` (Authenticode changes file bytes).
4. R2 publish prefers the signed artifact.

## Machine prerequisites (titane)

- Ethernet fixed IP `192.168.2.67`, WoL enabled (`wake-titane` from the Mac)
- `signtool.exe` on PATH (on titane: NuGet
  `Microsoft.Windows.SDK.BuildTools` under
  `C:\Tools\windows-sdk-buildtools\bin\10.0.26100.0\x64`)
- SimplySign Desktop / proCertum SmartSign, **logged in** in an interactive
  desktop session (TOTP from the mobile app; session ~2h)
- GitHub Actions runner at `C:\actions-runner` with labels:
  `self-hosted`, `Windows`, `X64`, `signing`
- Runner started with `C:\actions-runner\run.cmd` in that same interactive
  session (**not** installed as a Windows service), so it sees the user cert store
- Scheduled task `KeepAwakeDuringRemoteWork` while SSH / runner work is active

Certificate thumbprint (SHA1): `1D3E06AABF499CAA5C3F3FF0D4263B26698F7C7D`  
Timestamp: `http://time.certum.pl`

Local smoke sign:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File scripts/sign-windows-authenticode.ps1 -File path\to\setup.exe
```

## Security notes (public repo)

- Only `neolingua-hq/neolingua-center` may use the signing job
- Prefer waking titane only for release tags; leave the runner idle otherwise
- Do not expose the runner to untrusted `pull_request` workflows from forks
  (GitHub already avoids self-hosted for fork PRs by default)

## Operator checklist before a Windows release tag

1. `wake-titane`
2. Confirm SimplySign Desktop is unlocked (mobile TOTP if prompted)
3. Confirm the Actions runner shows **Idle** for titane
4. Push the SemVer tag (no `v` prefix)
