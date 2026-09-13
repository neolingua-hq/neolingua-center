#!/usr/bin/env node
/**
 * Bundle whisper.cpp CLI for Tauri (no Homebrew / Python at runtime).
 *
 * The ggml model is downloaded at runtime into app data, not into the installer.
 *
 * - macOS: build a mostly-static whisper-cli (Metal, no OpenMP) from a pinned tag
 * - Linux / Windows: build from source when cmake is available; otherwise download
 *   official release archives (shared libs must sit beside the CLI)
 */
import { execSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  copyFileSync,
  rmSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** Prefer a recent whisper.cpp tip; rebuild when models fail to load. */
const WHISPER_REF = "master";
const CLI_MIN_BYTES = 500_000;

const ROOT = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
);
const BINARIES = path.join(ROOT, "binaries");
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
  const built = findBuiltCli(build);
  if (!built) {
    throw new Error(
      `whisper-cli build missing under ${build} (checked bin/, bin/Release/, Release/, examples/cli/)`,
    );
  }
  return built;
}

/** Locate whisper-cli after cmake --build (Unix single-config vs MSVC multi-config). */
function findBuiltCli(build) {
  const exe = process.platform === "win32" ? "whisper-cli.exe" : "whisper-cli";
  const candidates = [
    path.join(build, "bin", exe),
    path.join(build, "bin", "Release", exe),
    path.join(build, "bin", "Debug", exe),
    path.join(build, "Release", exe),
    path.join(build, "Debug", exe),
    path.join(build, "examples", "cli", "Release", exe),
    path.join(build, "examples", "cli", exe),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate) && statSync(candidate).size >= CLI_MIN_BYTES) {
      return candidate;
    }
  }
  // Last resort: shallow walk under build/ for the executable name.
  try {
    const listed = execSync(
      process.platform === "win32"
        ? `powershell -NoProfile -Command "Get-ChildItem -Path ${JSON.stringify(build)} -Recurse -Filter ${JSON.stringify(exe)} -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName"`
        : `find ${JSON.stringify(build)} -name ${JSON.stringify(exe)} -type f 2>/dev/null`,
      { encoding: "utf8" },
    )
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean);
    for (const hit of listed) {
      if (existsSync(hit) && statSync(hit).size >= CLI_MIN_BYTES) {
        return hit;
      }
    }
  } catch {
    // ignore
  }
  return null;
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

mkdirSync(BINARIES, { recursive: true });
const wanted = process.argv.slice(2);
const triples = wanted.length > 0 ? wanted : [hostTriple()];
for (const triple of triples) {
  await ensureWhisperCli(triple);
}
