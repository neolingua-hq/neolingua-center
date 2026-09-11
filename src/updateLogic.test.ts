import { describe, expect, it } from "vitest";
import {
  HOUR_MS,
  escapeHtml,
  formatBytes,
  friendlyCheckError,
  friendlyUpdateError,
  parseLastCheckAt,
  shouldAutoInstallOnBoot,
  shouldClearStaleSchedule,
  shouldShowAvailableBanner,
  shouldThrottleAutoCheck,
} from "./updateLogic";

describe("parseLastCheckAt", () => {
  it("returns 0 for missing or junk values", () => {
    expect(parseLastCheckAt(null)).toBe(0);
    expect(parseLastCheckAt("")).toBe(0);
    expect(parseLastCheckAt("nope")).toBe(0);
  });

  it("parses a numeric timestamp", () => {
    expect(parseLastCheckAt("1700000000000")).toBe(1700000000000);
  });
});

describe("shouldThrottleAutoCheck", () => {
  const now = 2_000_000;
  const recent = now - 1000;

  it("throttles a background check within 1h", () => {
    expect(
      shouldThrottleAutoCheck({
        force: false,
        hasSchedule: false,
        lastCheckAt: recent,
        now,
      }),
    ).toBe(true);
  });

  it("does not throttle after 1h", () => {
    expect(
      shouldThrottleAutoCheck({
        force: false,
        hasSchedule: false,
        lastCheckAt: now - HOUR_MS,
        now,
      }),
    ).toBe(false);
  });

  it("never throttles a forced or scheduled check", () => {
    expect(
      shouldThrottleAutoCheck({
        force: true,
        hasSchedule: false,
        lastCheckAt: recent,
        now,
      }),
    ).toBe(false);
    expect(
      shouldThrottleAutoCheck({
        force: false,
        hasSchedule: true,
        lastCheckAt: recent,
        now,
      }),
    ).toBe(false);
  });
});

describe("schedule vs latest", () => {
  it("auto-installs only when the planned version is still latest", () => {
    expect(shouldAutoInstallOnBoot("0.1.1", "0.1.1")).toBe(true);
    expect(shouldAutoInstallOnBoot("0.1.1", "0.1.2")).toBe(false);
    expect(shouldAutoInstallOnBoot(null, "0.1.1")).toBe(false);
  });

  it("clears a stale schedule when latest differs", () => {
    expect(shouldClearStaleSchedule("0.1.1", "0.1.2")).toBe(true);
    expect(shouldClearStaleSchedule("0.1.1", "0.1.1")).toBe(false);
    expect(shouldClearStaleSchedule(null, "0.1.1")).toBe(false);
  });
});

describe("shouldShowAvailableBanner", () => {
  it("hides a dismissed version unless the check is forced", () => {
    expect(
      shouldShowAvailableBanner({
        force: false,
        dismissedVersion: "0.2.0",
        latest: "0.2.0",
      }),
    ).toBe(false);
    expect(
      shouldShowAvailableBanner({
        force: true,
        dismissedVersion: "0.2.0",
        latest: "0.2.0",
      }),
    ).toBe(true);
  });

  it("shows a new version even if another was dismissed", () => {
    expect(
      shouldShowAvailableBanner({
        force: false,
        dismissedVersion: "0.1.0",
        latest: "0.2.0",
      }),
    ).toBe(true);
  });
});

describe("formatBytes", () => {
  it("uses o, Ko, Mo", () => {
    expect(formatBytes(500)).toBe("500 o");
    expect(formatBytes(2048)).toBe("2 Ko");
    expect(formatBytes(1.5 * 1024 * 1024)).toBe("1.5 Mo");
  });
});

describe("escapeHtml", () => {
  it("escapes markup in notes", () => {
    expect(escapeHtml(`<b>"x" & y</b>`)).toBe("&lt;b&gt;&quot;x&quot; &amp; y&lt;/b&gt;");
  });
});

describe("friendly errors", () => {
  it("maps network failures without leaking stacks", () => {
    expect(friendlyUpdateError(new Error("network timeout"))).toBe(
      "Connexion impossible. Vérifie ton réseau et réessaie.",
    );
    expect(friendlyCheckError(new Error("fetch failed"))).toBe(
      "Connexion impossible. Vérifie ton réseau et réessaie.",
    );
  });

  it("maps signature and 404 cases", () => {
    expect(friendlyUpdateError("minisign signature mismatch")).toBe(
      "La mise à jour n’a pas pu être vérifiée. Réessaie plus tard.",
    );
    expect(friendlyUpdateError("404 not found")).toBe(
      "Mise à jour introuvable. Réessaie plus tard.",
    );
  });

  it("maps permission denied", () => {
    expect(friendlyUpdateError("permission denied")).toBe(
      "Installation refusée. Vérifie les droits sur l’application.",
    );
  });

  it("falls back to a short generic message", () => {
    expect(friendlyUpdateError(new Error("boom at updater.rs:12"))).toBe(
      "Impossible d’installer la mise à jour. Réessaie plus tard.",
    );
  });
});
