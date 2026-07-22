import { describe, expect, it } from "vitest";

import { cn } from "./cn";

describe("cn", () => {
  it("keeps a text color next to a custom font-size utility", () => {
    // Regression: tailwind-merge used to mistake the custom `text-nav` size for a
    // text COLOR and drop `text-white`, which made the dark button's label
    // invisible. Both must survive the merge.
    const result = cn("bg-ink text-white", "px-md py-sm text-nav");
    expect(result).toContain("text-white");
    expect(result).toContain("text-nav");
  });

  it("still lets a later color override an earlier one", () => {
    // The merge must keep working for genuine color conflicts.
    expect(cn("text-white", "text-ink-body")).toBe("text-ink-body");
  });

  it("keeps on-primary color with the button size", () => {
    const result = cn("bg-primary text-on-primary", "text-button");
    expect(result).toContain("text-on-primary");
    expect(result).toContain("text-button");
  });
});
