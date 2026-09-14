# Security

Please report vulnerabilities privately. Do not open a public issue for a
security problem.

Use GitHub’s private vulnerability reporting on this repository:

https://github.com/neolingua-hq/neolingua-center/security/advisories/new

Include enough detail to reproduce (OS, version, steps). We will acknowledge
and work on a fix before any public disclosure.

The updater minisign **public** key lives in `src-tauri/tauri.conf.json`. The
matching private key must never be committed. Signing material for Apple and
Cloudflare R2 lives in GitHub Actions secrets, not in this tree.
