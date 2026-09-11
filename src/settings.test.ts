import { describe, expect, it } from "vitest";
import { nextStep, prevStep, stepIndex, STEP_ORDER } from "./settings";

describe("wizard steps", () => {
  it("orders welcome → media → options → network", () => {
    expect(STEP_ORDER).toEqual(["welcome", "media", "options", "network"]);
  });

  it("walks forward and back", () => {
    expect(nextStep("welcome")).toBe("media");
    expect(nextStep("network")).toBeNull();
    expect(prevStep("welcome")).toBeNull();
    expect(prevStep("options")).toBe("media");
  });

  it("indexes known steps", () => {
    expect(stepIndex("options")).toBe(2);
    expect(stepIndex("welcome")).toBe(0);
  });
});
