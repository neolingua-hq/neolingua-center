#!/usr/bin/env node
/**
 * Bundle whisper.cpp CLI + ggml model for Tauri (no Homebrew / Python at runtime).
 *
 * - macOS: build a mostly-static whisper-cli (Metal, no OpenMP) from a pinned tag
 * - Linux / Windows: build from source when cmake is available; otherwise download
 *   official release archives (shared libs must sit beside the CLI)
 * - Model: ggml-small.bin as a Tauri resource
 */
import { execSync } from "node:child_process";
import {
  chmodSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  copyFileSync,
  rmSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { fileURLToPath } from "node:url";

/** Prefer a recent whisper.cpp tip; rebuild when models fail to load. */
const WHISPER_REF = "master";
const MODEL_NAME = "ggml-small.bin";
const MODEL_URL =
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
/** Exact Hugging Face Content-Length for ggml-small.bin */
const MODEL_EXPECTED_BYTES = 487_601_967;
const CLI_MIN_BYTES = 500_000;

const ROOT = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
);
const BINARIES = path.join(ROOT, "binaries");
const RESOURCES = path.join(ROOT, "resources", "whisper");
const BUILD_DIR = path.join(BINARIES, ".whisper-src");

function hostTriple() {
  return execSync("rustc --print host-tuple", { encoding: "utf8" }).trim();
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

async function downloadFile(url, dest) {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok || !res.body) {
    throw new Error(`Download failed ${url} (${res.status})`);
  }
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
}

function hasCmake() {
  try {
    execSync("cmake --version", { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function outCliName(triple) {
  const win = triple.includes("windows");
  return win ? `whisper-cli-${triple}.exe` : `whisper-cli-${triple}`;
}

function ensureRepo() {
  if (existsSync(path.join(BUILD_DIR, "CMakeLists.txt"))) {
    execSync(`git fetch --depth 1 origin ${WHISPER_REF}`, {
      cwd: BUILD_DIR,
      stdio: "inherit",
    });
    execSync(`git checkout -f FETCH_HEAD`, {
      cwd: BUILD_DIR,
      stdio: "inherit",
    });
    return;
  }
  mkdirSync(path.dirname(BUILD_DIR), { recursive: true });
  rmSync(BUILD_DIR, { recursive: true, force: true });
  execSync(
    `git clone --depth 1 --branch ${WHISPER_REF} https://github.com/ggml-org/whisper.cpp.git ${JSON.stringify(BUILD_DIR)}`,
    { stdio: "inherit", shell: true },
  );
}

function buildCli(triple) {
  if (!hasCmake()) {
    throw new Error(
      "cmake is required to build the bundled whisper-cli (install Xcode CLT / cmake).",
    );
  }
  ensureRepo();
  const build = path.join(BUILD_DIR, "build");
  const isDarwin = triple.includes("apple-darwin");
  const args = [
    "-B",
    build,
    `-DBUILD_SHARED_LIBS=OFF`,
    `-DGGML_OPENMP=OFF`,
    `-DGGML_NATIVE=OFF`,
    `-DCMAKE_BUILD_TYPE=Release`,
  ];
  if (isDarwin) {
    args.push(`-DGGML_METAL=ON`, `-DGGML_METAL_EMBED_LIBRARY=ON`);
    if (triple.startsWith("aarch64")) {
      args.push(`-DCMAKE_OSX_ARCHITECTURES=arm64`);
    } else if (triple.startsWith("x86_64")) {
      args.push(`-DCMAKE_OSX_ARCHITECTURES=x86_64`);
    }
  } else {
    args.push(`-DGGML_METAL=OFF`);
  }
  console.log(`cmake configure whisper.cpp ${WHISPER_REF}`);
  execSync(`cmake ${args.map((a) => JSON.stringify(a)).join(" ")}`, {
    cwd: BUILD_DIR,
    stdio: "inherit",
    shell: true,
  });
  const jobs =
    process.platform === "darwin" || process.platform === "linux"
      ? Number(execSync("getconf _NPROCESSORS_ONLN", { encoding: "utf8" }).trim()) || 4
      : 4;
  console.log(`cmake build whisper-cli (-j${jobs})`);
  execSync(`cmake --build ${JSON.stringify(build)} -j${jobs} --config Release --target whisper-cli`, {
    cwd: BUILD_DIR,
    stdio: "inherit",
    shell: true,
  });
  const built = path.join(build, "bin", process.platform === "win32" ? "whisper-cli.exe" : "whisper-cli");
  if (!existsSync(built)) {
    throw new Error(`whisper-cli build missing at ${built}`);
  }
  return built;
}

async function ensureWhisperCli(triple) {
  mkdirSync(BINARIES, { recursive: true });
  const outName = outCliName(triple);
  const out = path.join(BINARIES, outName);
  if (existsSync(out) && statSync(out).size >= CLI_MIN_BYTES) {
    console.log(`ok ${outName}`);
    return;
  }
  console.log(`build whisper-cli for ${triple}`);
  const built = buildCli(triple);
  copyFileSync(built, out);
  chmodSync(out, 0o755);
  adhocSign(out);
  console.log(`wrote ${outName} (${Math.round(statSync(out).size / 1e6)} MB)`);
}

async function ensureModel() {
  mkdirSync(RESOURCES, { recursive: true });
  const dest = path.join(RESOURCES, MODEL_NAME);
  if (existsSync(dest) && statSync(dest).size === MODEL_EXPECTED_BYTES) {
    console.log(`ok ${MODEL_NAME}`);
    return;
  }
  console.log(`fetch ${MODEL_NAME} (~465 MB)`);
  const partial = `${dest}.partial`;
  await downloadFile(MODEL_URL, partial);
  const size = existsSync(partial) ? statSync(partial).size : 0;
  if (size !== MODEL_EXPECTED_BYTES) {
    rmSync(partial, { force: true });
    throw new Error(
      `Model download size mismatch: got ${size}, expected ${MODEL_EXPECTED_BYTES}`,
    );
  }
  copyFileSync(partial, dest);
  rmSync(partial, { force: true });
  console.log(`wrote ${MODEL_NAME} (${Math.round(statSync(dest).size / 1e6)} MB)`);
}

mkdirSync(BINARIES, { recursive: true });
const wanted = process.argv.slice(2);
const triples = wanted.length > 0 ? wanted : [hostTriple()];
for (const triple of triples) {
  await ensureWhisperCli(triple);
}
await ensureModel();
