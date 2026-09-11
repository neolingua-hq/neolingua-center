/**
 * Shared media / subtitle helpers for Solo + Jam viewers.
 */

export const SIZE_STEPS = Object.freeze([0.9, 1.05, 1.2, 1.35, 1.55, 1.8]);

/** @param {unknown} value */
export function clampSizeIndex(value) {
  if (!Number.isFinite(value)) return 2;
  return Math.min(SIZE_STEPS.length - 1, Math.max(0, Math.round(/** @type {number} */ (value))));
}

/** @param {number} seconds */
export function formatClock(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) {
    return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  }
  return `${m}:${String(s).padStart(2, "0")}`;
}

/** Alias used by Solo / companion. */
export const formatTime = formatClock;

/** @param {unknown} text */
export function escapeHtml(text) {
  return String(text ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** @param {string} value */
export function parseTs(value) {
  const parts = value.trim().split(":");
  if (parts.length === 3) {
    return Number(parts[0]) * 3600 + Number(parts[1]) * 60 + Number(parts[2].replace(",", "."));
  }
  if (parts.length === 2) {
    return Number(parts[0]) * 60 + Number(parts[1].replace(",", "."));
  }
  return Number.NaN;
}

/**
 * @param {string} raw
 * @returns {{ start: number, end: number, text: string }[]}
 */
export function parseVtt(raw) {
  const blocks = String(raw).replace(/^\uFEFF/, "").replace(/\r/g, "").split(/\n\n+/);
  /** @type {{ start: number, end: number, text: string }[]} */
  const cues = [];
  for (const block of blocks) {
    const lines = block.split("\n").filter((line) => line.trim().length > 0);
    if (!lines.length) continue;
    if (lines[0].startsWith("WEBVTT") || lines[0].startsWith("NOTE")) continue;
    const timingIdx = lines[0].includes("-->") ? 0 : 1;
    if (!lines[timingIdx] || !lines[timingIdx].includes("-->")) continue;
    const match = /([\d:.]+)\s+-->\s+([\d:.]+)/.exec(lines[timingIdx]);
    if (!match) continue;
    const start = parseTs(match[1]);
    const end = parseTs(match[2]);
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) continue;
    const text = lines
      .slice(timingIdx + 1)
      .join("\n")
      .replace(/<[^>]+>/g, "")
      .trim();
    if (!text) continue;
    cues.push({ start, end, text });
  }
  return cues;
}

/**
 * Detect intro range from early subtitle gaps (cold open + theme, or theme at 0).
 * @param {{ start: number, end: number }[]} cues
 * @returns {{ start: number, end: number }}
 */
export function detectIntroRangeSeconds(cues) {
  const INTRO_MIN = 35;
  const INTRO_MAX = 75;
  const SEARCH_WINDOW = 8 * 60;
  const FALLBACK = 50;

  const early = cues.filter((c) => c.start <= SEARCH_WINDOW);
  /** @type {{ start: number, end: number, duration: number }[]} */
  const candidates = [];

  if (early.length === 0) return { start: 0, end: FALLBACK };

  const beforeFirst = early[0].start;
  if (beforeFirst >= INTRO_MIN && beforeFirst <= INTRO_MAX) {
    candidates.push({ start: 0, end: beforeFirst, duration: beforeFirst });
  }

  for (let i = 1; i < early.length; i += 1) {
    const start = early[i - 1].end;
    const end = early[i].start;
    const duration = end - start;
    if (duration >= INTRO_MIN && duration <= INTRO_MAX) {
      candidates.push({ start, end, duration });
    }
  }

  if (candidates.length > 0) {
    const best = candidates.sort((a, b) => b.duration - a.duration)[0];
    return { start: best.start, end: best.end };
  }

  let bestStart = 0;
  let bestEnd = Math.min(beforeFirst || FALLBACK, FALLBACK);
  let bestDuration = bestEnd;

  for (let i = 1; i < early.length; i += 1) {
    const start = early[i - 1].end;
    const end = early[i].start;
    const duration = end - start;
    if (duration > bestDuration) {
      bestDuration = duration;
      bestStart = start;
      bestEnd = end;
    }
  }

  if (beforeFirst > bestDuration) {
    return { start: 0, end: beforeFirst };
  }
  return { start: bestStart, end: bestEnd };
}

/**
 * Detect intro end from early subtitle gaps.
 * @param {{ start: number, end: number }[]} cues
 * @returns {number}
 */
export function detectIntroEndSeconds(cues) {
  return detectIntroRangeSeconds(cues).end;
}
