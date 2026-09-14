#!/usr/bin/env node
/**
 * Rust fmt / Clippy. Clippy needs Tauri sidecars (ffmpeg); skip with a
 * warning when they are missing so a first clone can still commit TS-only work.
 */
import { execSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const mode = process.argv[2] ?? "check";

function run(command) {
  execSync(command, { stdio: "inherit", cwd: ROOT, shell: true });
}

function hasSidecars() {
  const triple = execSync("rustc --print host-tuple", {
    encoding: "utf8",
    cwd: ROOT,
  }).trim();
  const win = triple.includes("windows");
  const name = win ? `ffmpeg-${triple}.exe` : `ffmpeg-${triple}`;
  return existsSync(path.join(ROOT, "src-tauri", "binaries", name));
}

if (mode === "fmt") {
  run("cargo fmt --manifest-path src-tauri/Cargo.toml");
} else if (mode === "fmt-check") {
  run("cargo fmt --manifest-path src-tauri/Cargo.toml -- --check");
} else if (mode === "clippy") {
  if (!hasSidecars()) {
    console.warn(
      "skip clippy: sidecars missing (npm run fetch-binaries), then retry",
    );
    process.exit(0);
  }
  run("cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings");
} else {
  console.error(`usage: lint-rs.mjs fmt|fmt-check|clippy`);
  process.exit(1);
}
