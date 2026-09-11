import { describe, expect, it } from "vitest";
import {
  detectIntroEndSeconds,
  detectIntroRangeSeconds,
  escapeHtml,
  formatClock,
  parseTs,
  parseVtt,
} from "./media-utils.js";

describe("parseVtt / parseTs", () => {
  it("parses cues and strips tags", () => {
    const cues = parseVtt(`WEBVTT

00:01.000 --> 00:02.500
Hello <i>world</i>

00:03.000 --> 00:04.000
Bye
`);
    expect(cues).toEqual([
      { start: 1, end: 2.5, text: "Hello world" },
      { start: 3, end: 4, text: "Bye" },
    ]);
  });

  it("parses hh:mm:ss", () => {
    expect(parseTs("01:02:03.5")).toBe(3723.5);
  });
});

describe("formatClock / escapeHtml / intro", () => {
  it("formats with hours when needed", () => {
    expect(formatClock(65)).toBe("1:05");
    expect(formatClock(3661)).toBe("1:01:01");
  });

  it("escapes html", () => {
    expect(escapeHtml(`a<"&`)).toBe("a&lt;&quot;&amp;");
  });

  it("detects intro from early gap", () => {
    const end = detectIntroEndSeconds([
      { start: 0, end: 5 },
      { start: 55, end: 60 },
    ]);
    expect(end).toBe(55);
  });

  it("detects cold-open intro range (start + end)", () => {
    const range = detectIntroRangeSeconds([
      { start: 2, end: 5 },
      { start: 50, end: 56.9 },
      { start: 109.3, end: 112 },
    ]);
    expect(range).toEqual({ start: 56.9, end: 109.3 });
  });

  it("detects intro at episode start", () => {
    const range = detectIntroRangeSeconds([
      { start: 48, end: 52 },
      { start: 53, end: 56 },
    ]);
    expect(range).toEqual({ start: 0, end: 48 });
  });
});
