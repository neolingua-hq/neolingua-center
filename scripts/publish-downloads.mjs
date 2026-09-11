#!/usr/bin/env node
/**
 * Upload Neolingua Center installers to R2 (download.neolingua.app).
 *
 * Overwrites stable latest.* keys and uploads versioned copies.
 *
 * Auth (S3-compatible API, required for files > ~300 MiB):
 *   CLOUDFLARE_ACCOUNT_ID
 *   R2_ACCESS_KEY_ID
 *   R2_SECRET_ACCESS_KEY
 *
 * Optional: load the same vars from `.secrets/r2.env` (gitignored).
 * Create a token: Cloudflare Dashboard → R2 → Manage R2 API Tokens
 *   (Object Read & Write on bucket `neolingua-downloads`).
 *
 * Usage (after a signed `npm run tauri build`):
 *   npm run publish-downloads
 */
import { spawnSync } from "node:child_process";
import {
  existsSync,
  readdirSync,
  readFileSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const BUNDLE = path.join(ROOT, "src-tauri", "target", "release", "bundle");
const BUCKET = "neolingua-downloads";
const SECRETS_ENV = path.join(ROOT, ".secrets", "r2.env");
const WRANGLER_MAX_BYTES = 280 * 1024 * 1024;

loadDotEnv(SECRETS_ENV);

const pkg = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8"));
const version = String(pkg.version || "").trim();
if (!version) {
  console.error("package.json is missing version");
  process.exit(1);
}

/**
 * @param {string} filePath
 */
function loadDotEnv(filePath) {
  if (!existsSync(filePath)) return;
  for (const line of readFileSync(filePath, "utf8").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const eq = trimmed.indexOf("=");
    if (eq < 0) continue;
    const key = trimmed.slice(0, eq).trim();
    let value = trimmed.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    if (!(key in process.env) || !process.env[key]) {
      process.env[key] = value;
    }
  }
}

/**
 * @param {string} dir
 * @param {RegExp} re
 * @returns {string | null}
 */
function findArtifact(dir, re) {
  if (!existsSync(dir)) return null;
  const matches = readdirSync(dir)
    .filter((name) => re.test(name))
    .map((name) => path.join(dir, name))
    .filter((file) => statSync(file).isFile())
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  return matches[0] ?? null;
}

function requireS3Env() {
  const accountId = process.env.CLOUDFLARE_ACCOUNT_ID?.trim();
  const accessKeyId = process.env.R2_ACCESS_KEY_ID?.trim();
  const secretAccessKey = process.env.R2_SECRET_ACCESS_KEY?.trim();
  if (!accountId || !accessKeyId || !secretAccessKey) {
    throw new Error(
      "Large uploads need R2 S3 credentials.\n" +
        "Set CLOUDFLARE_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY\n" +
        "or create neolingua-center/.secrets/r2.env with those keys.\n" +
        "Dashboard: https://dash.cloudflare.com/?to=/:account/r2/api-tokens",
    );
  }
  return { accountId, accessKeyId, secretAccessKey };
}

/**
 * @param {string} key
 * @param {string} file
 * @param {string} contentType
 */
function putViaWrangler(key, file, contentType) {
  const result = spawnSync(
    "npx",
    [
      "--yes",
      "wrangler",
      "r2",
      "object",
      "put",
      `${BUCKET}/${key}`,
      "--file",
      file,
      "--content-type",
      contentType,
      "--remote",
    ],
    { stdio: "inherit", cwd: ROOT, encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error(`wrangler upload failed for ${key}`);
  }
}

/**
 * Multipart-capable upload via AWS CLI → R2 S3 endpoint.
 * @param {string} key
 * @param {string} file
 * @param {string} contentType
 */
function putViaS3(key, file, contentType) {
  const { accountId, accessKeyId, secretAccessKey } = requireS3Env();
  const endpoint = `https://${accountId}.r2.cloudflarestorage.com`;
  const result = spawnSync(
    "aws",
    [
      "s3",
      "cp",
      file,
      `s3://${BUCKET}/${key}`,
      "--endpoint-url",
      endpoint,
      "--content-type",
      contentType,
      "--no-progress",
    ],
    {
      stdio: "inherit",
      cwd: ROOT,
      env: {
        ...process.env,
        AWS_ACCESS_KEY_ID: accessKeyId,
        AWS_SECRET_ACCESS_KEY: secretAccessKey,
        AWS_DEFAULT_REGION: "auto",
        AWS_EC2_METADATA_DISABLED: "true",
      },
    },
  );
  if (result.status !== 0) {
    throw new Error(`aws s3 cp failed for ${key}`);
  }
}

/**
 * @param {string} key
 * @param {string} file
 * @param {string} contentType
 */
function putObject(key, file, contentType) {
  const size = statSync(file).size;
  if (size < 1024) {
    throw new Error(`Refusing to upload tiny file (${size} bytes): ${file}`);
  }
  const mib = (size / (1024 * 1024)).toFixed(1);
  console.log(`→ r2://${BUCKET}/${key} (${mib} MiB)`);
  if (size > WRANGLER_MAX_BYTES) {
    console.log("  (multipart via R2 S3 API)");
    putViaS3(key, file, contentType);
  } else {
    try {
      putViaWrangler(key, file, contentType);
    } catch (err) {
      console.warn(String(err));
      console.log("  falling back to R2 S3 API");
      putViaS3(key, file, contentType);
    }
  }
}

const dmg = findArtifact(path.join(BUNDLE, "dmg"), /\.dmg$/i);
const exe = findArtifact(path.join(BUNDLE, "nsis"), /\.exe$/i);

if (!dmg && !exe) {
  console.error(
    "No installers found under src-tauri/target/release/bundle/{dmg,nsis}.\n" +
      "Run a signed `npm run tauri build` first (targets: app, dmg, nsis).",
  );
  process.exit(1);
}

const arch =
  process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x64" : process.arch;

try {
  if (dmg) {
    putObject("macos/latest.dmg", dmg, "application/x-apple-diskimage");
    putObject(
      `macos/Neolingua-Center-${version}-${arch}.dmg`,
      dmg,
      "application/x-apple-diskimage",
    );
  }

  if (exe) {
    putObject("windows/latest.exe", exe, "application/octet-stream");
    putObject(
      `windows/Neolingua-Center-${version}-x64.exe`,
      exe,
      "application/octet-stream",
    );
  }
} catch (err) {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
}

console.log("Done.");
console.log("  https://download.neolingua.app/macos/latest.dmg");
console.log("  https://download.neolingua.app/windows/latest.exe");
