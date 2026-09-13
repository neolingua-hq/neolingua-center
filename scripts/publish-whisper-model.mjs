#!/usr/bin/env node
/**
 * Publish ggml-small.bin to R2 (download.neolingua.app/whisper/).
 * Idempotent: skips when the object already has the expected size.
 *
 * Auth (same as installer uploads):
 *   CLOUDFLARE_ACCOUNT_ID
 *   R2_ACCESS_KEY_ID
 *   R2_SECRET_ACCESS_KEY
 * Optional: neolingua-center/.secrets/r2.env
 */
import { spawnSync } from "node:child_process";
import {
  createWriteStream,
  existsSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const BUCKET = "neolingua-downloads";
const KEY = "whisper/ggml-small.bin";
const MODEL_NAME = "ggml-small.bin";
const MODEL_EXPECTED_BYTES = 487_601_967;
const SOURCE_URL =
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
const LOCAL_MODEL = path.join(
  ROOT,
  "src-tauri",
  "resources",
  "whisper",
  MODEL_NAME,
);
const SECRETS_ENV = path.join(ROOT, ".secrets", "r2.env");

loadDotEnv(SECRETS_ENV);

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

function requireS3Env() {
  const accountId = process.env.CLOUDFLARE_ACCOUNT_ID?.trim();
  const accessKeyId = process.env.R2_ACCESS_KEY_ID?.trim();
  const secretAccessKey = process.env.R2_SECRET_ACCESS_KEY?.trim();
  if (!accountId || !accessKeyId || !secretAccessKey) {
    throw new Error(
      "R2 S3 credentials required (CLOUDFLARE_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY).",
    );
  }
  return { accountId, accessKeyId, secretAccessKey };
}

function s3Env() {
  const { accountId, accessKeyId, secretAccessKey } = requireS3Env();
  return {
    accountId,
    env: {
      ...process.env,
      AWS_ACCESS_KEY_ID: accessKeyId,
      AWS_SECRET_ACCESS_KEY: secretAccessKey,
      AWS_DEFAULT_REGION: "auto",
      AWS_EC2_METADATA_DISABLED: "true",
    },
    endpoint: `https://${accountId}.r2.cloudflarestorage.com`,
  };
}

function remoteSize() {
  const { env, endpoint } = s3Env();
  const result = spawnSync(
    "aws",
    [
      "s3api",
      "head-object",
      "--bucket",
      BUCKET,
      "--key",
      KEY,
      "--endpoint-url",
      endpoint,
      "--output",
      "json",
    ],
    { encoding: "utf8", env },
  );
  if (result.status !== 0) return null;
  try {
    const parsed = JSON.parse(result.stdout || "{}");
    const size = Number(parsed.ContentLength);
    return Number.isFinite(size) ? size : null;
  } catch {
    return null;
  }
}

async function downloadSource(dest) {
  console.log(`fetch ${MODEL_NAME} from Hugging Face (~465 MB)`);
  const res = await fetch(SOURCE_URL, { redirect: "follow" });
  if (!res.ok || !res.body) {
    throw new Error(`Download failed ${SOURCE_URL} (${res.status})`);
  }
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
}

function upload(file) {
  const { env, endpoint } = s3Env();
  const result = spawnSync(
    "aws",
    [
      "s3",
      "cp",
      file,
      `s3://${BUCKET}/${KEY}`,
      "--endpoint-url",
      endpoint,
      "--content-type",
      "application/octet-stream",
      "--cache-control",
      "public, max-age=31536000, immutable",
    ],
    { stdio: "inherit", cwd: ROOT, env },
  );
  if (result.status !== 0) {
    throw new Error(`aws s3 cp failed for ${KEY}`);
  }
}

const existing = remoteSize();
if (existing === MODEL_EXPECTED_BYTES) {
  console.log(`ok r2://${BUCKET}/${KEY} (${existing} bytes)`);
  process.exit(0);
}

try {
  let file = LOCAL_MODEL;
  let cleanup = null;
  if (!(existsSync(file) && statSync(file).size === MODEL_EXPECTED_BYTES)) {
    file = path.join(tmpdir(), `neolingua-${MODEL_NAME}`);
    cleanup = file;
    await downloadSource(file);
  }

  if (statSync(file).size !== MODEL_EXPECTED_BYTES) {
    if (cleanup) rmSync(cleanup, { force: true });
    throw new Error(
      `Model size mismatch: got ${statSync(file).size}, expected ${MODEL_EXPECTED_BYTES}`,
    );
  }

  console.log(`→ r2://${BUCKET}/${KEY} (${(MODEL_EXPECTED_BYTES / 1e6).toFixed(0)} MB)`);
  upload(file);
  if (cleanup) rmSync(cleanup, { force: true });
  console.log("https://download.neolingua.app/whisper/ggml-small.bin");
} catch (err) {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
}
