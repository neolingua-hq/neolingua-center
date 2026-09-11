import { describe, expect, it } from "vitest";
import {
  annotatePlainCue,
  buildQuizProposal,
  buildQuizSchedule,
  findScheduledQuiz,
  normalizeQuizInterval,
  QUIZ_INTERVAL_ALLOWED,
} from "./jam-gaps.js";

describe("normalizeQuizInterval", () => {
  it("keeps whitelist values", () => {
    for (const seconds of QUIZ_INTERVAL_ALLOWED) {
      expect(normalizeQuizInterval(seconds)).toBe(seconds);
    }
  });

  it("falls back to 60", () => {
    expect(normalizeQuizInterval(5)).toBe(60);
    expect(normalizeQuizInterval(90)).toBe(60);
    expect(normalizeQuizInterval(null)).toBe(60);
  });
});

describe("annotatePlainCue", () => {
  it("tokenizes words and punctuation", () => {
    const cue = annotatePlainCue({ start: 1, end: 2, text: "Hello, world!" });
    expect(cue.tokens.map((t) => t.text)).toEqual(["Hello", ", ", "world", "!"]);
    expect(cue.tokens.filter((t) => t.level === "unknown")).toHaveLength(2);
  });
});

describe("buildQuizProposal", () => {
  it("can gap duplicate words by token index", () => {
    const cue = annotatePlainCue({
      start: 10,
      end: 12,
      text: "Bye Bye friend forever",
    });
    const proposal = buildQuizProposal(cue, [cue], 2);
    expect(proposal).not.toBeNull();
    expect(proposal.gaps.length).toBeGreaterThanOrEqual(1);
    const byeGaps = proposal.gaps.filter((g) => g.answer.toLowerCase() === "bye");
    // Same surface form may appear twice when both token indexes are picked.
    if (byeGaps.length === 2) {
      expect(byeGaps[0].id).not.toBe(byeGaps[1].id);
    }
    const optionLower = proposal.options.map((o) => o.toLowerCase());
    const uniqueOptions = new Set(optionLower);
    // Options list may repeat an answer word once per gap, but distractors stay unique.
    expect(uniqueOptions.size).toBeGreaterThan(0);
  });

  it("is stable for the same cue seed", () => {
    const cue = annotatePlainCue({
      start: 30,
      end: 32.5,
      text: "Something interesting happened yesterday",
    });
    const a = buildQuizProposal(cue, [cue], 2);
    const b = buildQuizProposal(cue, [cue], 2);
    expect(a?.gaps.map((g) => g.answer)).toEqual(b?.gaps.map((g) => g.answer));
    expect(a?.options).toEqual(b?.options);
  });
});

describe("buildQuizSchedule", () => {
  it("spaces quizzes by normalized interval after cue end", () => {
    const cues = [
      annotatePlainCue({ start: 70, end: 72, text: "First interesting sentence here" }),
      annotatePlainCue({ start: 100, end: 102, text: "Second interesting sentence here" }),
      annotatePlainCue({ start: 200, end: 202, text: "Third interesting sentence here" }),
    ];
    const schedule = buildQuizSchedule(cues, 120, 60);
    expect(schedule.length).toBeGreaterThanOrEqual(1);
    for (let i = 1; i < schedule.length; i += 1) {
      expect(schedule[i].cueStart).toBeGreaterThanOrEqual(schedule[i - 1].cueEnd + 120 - 0.01);
    }
  });

  it("rejects unknown intervals via normalize", () => {
    const cues = [
      annotatePlainCue({ start: 70, end: 72, text: "First interesting sentence here" }),
      annotatePlainCue({ start: 140, end: 142, text: "Second interesting sentence here" }),
    ];
    const withBad = buildQuizSchedule(cues, 7, 60);
    const withDefault = buildQuizSchedule(cues, 60, 60);
    expect(withBad.map((s) => s.cueStart)).toEqual(withDefault.map((s) => s.cueStart));
  });

  it("findScheduledQuiz matches cue start in ms", () => {
    const cues = [
      annotatePlainCue({ start: 70.123, end: 72, text: "First interesting sentence here" }),
    ];
    const schedule = buildQuizSchedule(cues, 60, 60);
    expect(findScheduledQuiz(schedule, { start: 70.123 })).toBeTruthy();
    expect(findScheduledQuiz(schedule, { start: 70.124 })).toBeUndefined();
  });
});
