/**
 * Jam quiz gap builder (ported from poc/client/src/jam-gaps.ts).
 * Center has plain EN VTT (no CEFR annotations): words are tokenized and
 * treated as mid/unknown difficulty for blank picking + distractors.
 */

import { escapeHtml } from "./media-utils.js";

export const QUIZ_INTERVAL_SECONDS = 60;
export const QUIZ_FIRST_AFTER_SECONDS = 60;

/** Allowed Jam quiz intervals (seconds). Keep in sync with Rust `db::QUIZ_INTERVAL_SECONDS`. */
export const QUIZ_INTERVAL_ALLOWED = Object.freeze([30, 60, 120, 300, 900]);

export const QUIZ_INTERVAL_OPTIONS = Object.freeze([
  { seconds: 30, label: "30 secondes" },
  { seconds: 60, label: "1 minute" },
  { seconds: 120, label: "2 minutes" },
  { seconds: 300, label: "5 minutes" },
  { seconds: 900, label: "15 minutes" },
]);

/** @param {unknown} raw */
export function normalizeQuizInterval(raw) {
  const value = Number(raw);
  if (QUIZ_INTERVAL_ALLOWED.some((seconds) => seconds === value)) return value;
  return QUIZ_INTERVAL_SECONDS;
}

const STOP = new Set([
  "a",
  "an",
  "the",
  "and",
  "or",
  "but",
  "to",
  "of",
  "in",
  "on",
  "at",
  "for",
  "is",
  "are",
  "was",
  "were",
  "be",
  "been",
  "i",
  "you",
  "he",
  "she",
  "it",
  "we",
  "they",
  "my",
  "your",
  "his",
  "her",
  "our",
  "their",
  "this",
  "that",
  "with",
  "as",
  "from",
  "by",
  "not",
  "no",
  "yes",
  "me",
  "him",
  "us",
  "them",
  "do",
  "does",
  "did",
  "have",
  "has",
  "had",
  "will",
  "would",
  "can",
  "could",
  "should",
  "just",
  "so",
  "if",
  "oh",
  "hey",
  "okay",
  "ok",
  "uh",
  "um",
]);

/** @param {string} text */
function cleanWord(text) {
  return text.replace(/^[^a-zA-Z']+|[^a-zA-Z']+$/g, "");
}

/**
 * Build a cue with character-offset tokens from a plain VTT line.
 * @param {{ start: number, end: number, text: string }} cue
 */
export function annotatePlainCue(cue) {
  const text = String(cue.text || "");
  /** @type {{ text: string, level: string, note: null, start: number, end: number }[]} */
  const tokens = [];
  const re = /[A-Za-z']+|[^A-Za-z']+/g;
  let match;
  while ((match = re.exec(text)) !== null) {
    const value = match[0];
    const isWord = /[A-Za-z]/.test(value);
    tokens.push({
      text: value,
      level: isWord ? "unknown" : "skip",
      note: null,
      start: match.index,
      end: match.index + value.length,
    });
  }
  return {
    start: cue.start,
    end: cue.end,
    text,
    tokens,
  };
}

/** @param {unknown} token */
function isWordToken(token) {
  return (
    token &&
    typeof token === "object" &&
    /[a-zA-Z]/.test(/** @type {{ text?: string }} */ (token).text || "") &&
    /** @type {{ level?: string }} */ (token).level !== "skip"
  );
}

/** Length band when CEFR is unavailable. */
function bandOfWord(word) {
  if (word.length >= 8) return "hard";
  if (word.length >= 5) return "mid";
  return "easy";
}

/** @template T @param {T[]} items @param {number} seed */
function seededShuffle(items, seed) {
  const copy = [...items];
  let state = seed >>> 0 || 1;
  const next = () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 0x100000000;
  };
  for (let i = copy.length - 1; i > 0; i -= 1) {
    const j = Math.floor(next() * (i + 1));
    [copy[i], copy[j]] = [copy[j], copy[i]];
  }
  return copy;
}

/** @param {{ start: number, text: string }} cue */
function cueSeed(cue) {
  const t = Math.round(cue.start * 1000);
  let h = t ^ (cue.text.length * 2654435761);
  for (let i = 0; i < Math.min(cue.text.length, 24); i += 1) {
    h = Math.imul(h ^ cue.text.charCodeAt(i), 16777619);
  }
  return h >>> 0;
}

/**
 * @param {{ tokens: { text: string, level: string, start: number, end: number }[] }} cue
 * @param {number} [maxGaps]
 */
function collectCandidates(cue) {
  /** @type {{ token: object, index: number, band: string, word: string }[]} */
  const out = [];
  cue.tokens.forEach((token, index) => {
    if (!isWordToken(token)) return;
    const word = cleanWord(token.text);
    if (word.length < 3) return;
    if (STOP.has(word.toLowerCase())) return;
    out.push({ token, index, band: bandOfWord(word), word });
  });
  return out;
}

/**
 * @param {{ start: number, text: string, tokens: object[] }} cue
 * @param {number} [maxGaps]
 */
function pickGapCandidates(cue, maxGaps = 2) {
  const candidates = collectCandidates(cue);
  if (candidates.length === 0) return [];

  const seed = cueSeed(cue);
  /** @type {Record<string, typeof candidates>} */
  const byBand = {
    easy: seededShuffle(
      candidates.filter((c) => c.band === "easy"),
      seed ^ 0x1111,
    ),
    mid: seededShuffle(
      candidates.filter((c) => c.band === "mid"),
      seed ^ 0x2222,
    ),
    hard: seededShuffle(
      candidates.filter((c) => c.band === "hard"),
      seed ^ 0x3333,
    ),
  };

  /** @type {typeof candidates} */
  const picked = [];
  const usedBands = new Set();

  for (const band of ["hard", "mid", "easy"]) {
    if (picked.length >= maxGaps) break;
    const next = byBand[band].find((c) => !picked.some((p) => p.index === c.index));
    if (!next) continue;
    picked.push(next);
    usedBands.add(band);
  }

  if (picked.length < maxGaps) {
    for (const band of ["hard", "mid", "easy"]) {
      if (picked.length >= maxGaps) break;
      if (usedBands.has(band) && picked.length > 0) continue;
      const next = byBand[band].find((c) => !picked.some((p) => p.index === c.index));
      if (!next) continue;
      picked.push(next);
      usedBands.add(band);
    }
  }

  if (picked.length === 0) {
    picked.push(...seededShuffle(candidates, seed).slice(0, 1));
  } else if (picked.length < maxGaps) {
    for (const c of seededShuffle(candidates, seed ^ 0xabcd)) {
      if (picked.length >= maxGaps) break;
      if (picked.some((p) => p.index === c.index)) continue;
      picked.push(c);
    }
  }

  picked.sort((a, b) => a.index - b.index);
  return picked;
}

/**
 * @param {{ start: number, end: number, text: string, tokens: { text: string, start: number, end: number }[] }} cue
 * @param {typeof cue[]} allCues
 * @param {number} [maxGaps]
 */
export function buildQuizProposal(cue, allCues, maxGaps = 2) {
  const picked = pickGapCandidates(cue, maxGaps);
  if (picked.length === 0) return null;

  /** @type {Map<number, string>} */
  const gapByTokenIndex = new Map();
  /** @type {{ id: string, answer: string, level: string }[]} */
  const gaps = [];
  picked.forEach((item, i) => {
    const id = `g${i}`;
    gapByTokenIndex.set(item.index, id);
    gaps.push({ id, answer: item.word, level: "unknown" });
  });

  /** @type {({ type: "text", value: string } | { type: "gap", id: string })[]} */
  const segments = [];
  let cursor = 0;
  cue.tokens.forEach((token, index) => {
    if (token.start > cursor) {
      segments.push({ type: "text", value: cue.text.slice(cursor, token.start) });
    }
    const gapId = gapByTokenIndex.get(index);
    if (gapId) {
      segments.push({ type: "gap", id: gapId });
    } else {
      segments.push({ type: "text", value: token.text });
    }
    cursor = Math.max(cursor, token.end);
  });
  if (cursor < cue.text.length) {
    segments.push({ type: "text", value: cue.text.slice(cursor) });
  }

  const answers = new Set(gaps.map((g) => g.answer.toLowerCase()));
  /** @type {string[]} */
  const distractors = [];
  const seed = cueSeed(cue);
  for (const other of seededShuffle(allCues, seed ^ 0x55aa).slice(0, 40)) {
    for (const token of other.tokens) {
      if (!isWordToken(token)) continue;
      const word = cleanWord(token.text);
      if (word.length < 3) continue;
      if (STOP.has(word.toLowerCase())) continue;
      if (answers.has(word.toLowerCase())) continue;
      if (distractors.some((d) => d.toLowerCase() === word.toLowerCase())) continue;
      distractors.push(word);
      if (distractors.length >= 8) break;
    }
    if (distractors.length >= 8) break;
  }

  const options = seededShuffle(
    [
      ...gaps.map((g) => g.answer),
      ...seededShuffle(distractors, seed ^ 0x7777).slice(0, Math.max(3, 6 - gaps.length)),
    ],
    seed ^ 0x9999,
  );

  return { segments, gaps, options };
}

/**
 * @param {{ start: number, end: number, text: string, tokens: object[] }[]} cues
 * @param {number} intervalSeconds
 * @param {number} [firstAfterSeconds]
 */
export function buildQuizSchedule(
  cues,
  intervalSeconds,
  firstAfterSeconds = QUIZ_FIRST_AFTER_SECONDS,
) {
  const interval = normalizeQuizInterval(intervalSeconds);
  const sorted = [...cues].sort((a, b) => a.start - b.start || a.end - b.end);
  /** @type {{ cueStart: number, cueEnd: number, proposal: NonNullable<ReturnType<typeof buildQuizProposal>> }[]} */
  const schedule = [];
  let nextEligibleAt = Math.max(0, firstAfterSeconds);

  for (const cue of sorted) {
    if (cue.end - cue.start < 0.8) continue;
    if (cue.end + 0.05 < nextEligibleAt) continue;
    const proposal = buildQuizProposal(cue, cues);
    if (!proposal || proposal.gaps.length === 0) continue;
    schedule.push({
      cueStart: cue.start,
      cueEnd: cue.end,
      gapTokenIndexes: pickGapCandidates(cue).map((c) => c.index),
      proposal,
    });
    nextEligibleAt = cue.end + interval;
  }

  return schedule;
}

/**
 * @param {{ cueStart: number }[]} schedule
 * @param {{ start: number }} cue
 */
export function findScheduledQuiz(schedule, cue) {
  const key = Math.round(cue.start * 1000);
  return schedule.find((item) => Math.round(item.cueStart * 1000) === key);
}

/**
 * Render cue HTML with optional blanked token indexes.
 * @param {{ text: string, tokens: { text: string, start: number, end: number }[] }} cue
 * @param {Set<number> | number[] | null} [maskedIndexes]
 */
export function renderGappedCueHtml(cue, maskedIndexes = null) {
  const masked =
    maskedIndexes instanceof Set
      ? maskedIndexes
      : maskedIndexes
        ? new Set(maskedIndexes)
        : null;
  let html = "";
  let cursor = 0;
  cue.tokens.forEach((token, index) => {
    if (token.start > cursor) {
      html += escapeHtml(cue.text.slice(cursor, token.start));
    }
    if (masked?.has(index)) {
      const word = cleanWord(token.text);
      const blank = "_".repeat(Math.min(8, Math.max(3, word.length || 3)));
      html += `<span class="jam-sub-gap">${blank}</span>`;
    } else {
      html += escapeHtml(token.text);
    }
    cursor = Math.max(cursor, token.end);
  });
  if (cursor < cue.text.length) {
    html += escapeHtml(cue.text.slice(cursor));
  }
  return html;
}
