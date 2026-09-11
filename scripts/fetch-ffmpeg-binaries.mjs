#!/usr/bin/env node
/**
 * Download self-contained ffmpeg/ffprobe sidecars for Tauri externalBin.
 * Source: eugeneware/ffmpeg-static (system frameworks only, no Homebrew).
 */
import { execSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, chmodSync, statSync } from "node:fs";
import path from "node:path";
import { pipeline } from "node:stream/promises";
import { createGunzip } from "node:zlib";
import { Readable } from "node:stream";
import { fileURLToPath } from "node:url";

const VERSION = "b6.1.1";
const MIN_BYTES = 1_000_000;
const ROOT = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
  "binaries",
);

/** rustc host-tuple -> eugeneware asset suffix */
const PLATFORM_MAP = {
  "aarch64-apple-darwin": "darwin-arm64",
  "x86_64-apple-darwin": "darwin-x64",
  "x86_64-pc-windows-msvc": "win32-x64",
  "aarch64-unknown-linux-gnu": "linux-arm64",
  "x86_64-unknown-linux-gnu": "linux-x64",
};

function hostTriple() {
  return execSync("rustc --print host-tuple", { encoding: "utf8" }).trim();
}

async function downloadGzip(url, dest) {
  const res = await fetch(url);
  if (!res.ok || !res.body) {
    throw new Error(`Download failed ${url} (${res.status})`);
  }
  await pipeline(Readable.fromWeb(res.body), createGunzip(), createWriteStream(dest));
  chmodSync(dest, 0o755);
}

function adhocSign(filePath) {
  if (process.platform !== "darwin") return;
  try {
    execSync(`codesign --force --sign - ${JSON.stringify(filePath)}`, {
      stdio: "inherit",
    });
  } catch {
    // Unsigned local builds still work for development.
  }
}

async function ensure(tool, triple) {
  const vendor = PLATFORM_MAP[triple];
  if (!vendor) {
    console.warn(`skip ${tool}: unsupported triple ${triple}`);
    return;
  }
  const win = triple.includes("windows");
  const outName = win ? `${tool}-${triple}.exe` : `${tool}-${triple}`;
  const out = path.join(ROOT, outName);
  if (existsSync(out) && statSync(out).size >= MIN_BYTES) {
    console.log(`ok ${outName}`);
    return;
  }
  const url = `https://github.com/eugeneware/ffmpeg-static/releases/download/${VERSION}/${tool}-${vendor}.gz`;
  console.log(`fetch ${tool}-${vendor}`);
  await downloadGzip(url, out);
  adhocSign(out);
  console.log(`wrote ${outName} (${Math.round(statSync(out).size / 1e6)} MB)`);
}

mkdirSync(ROOT, { recursive: true });
const wanted = process.argv.slice(2);
const triples = wanted.length > 0 ? wanted : [hostTriple()];
for (const triple of triples) {
  await ensure("ffmpeg", triple);
  await ensure("ffprobe", triple);
}
