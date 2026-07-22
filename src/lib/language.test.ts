import { describe, expect, it } from "vitest";

import { needsTranslation } from "./language";

describe("needsTranslation", () => {
  it("flags an English page for a Korean reader", () => {
    const english =
      "The dominant sequence transduction models are based on complex recurrent networks.";
    expect(needsTranslation(english, "ko")).toBe(true);
  });

  it("does not flag a Korean page for a Korean reader", () => {
    const korean = "지배적인 시퀀스 변환 모델은 복잡한 순환 신경망에 기반한다. 우리는 트랜스포머를 제안한다.";
    expect(needsTranslation(korean, "ko")).toBe(false);
  });

  it("flags a Korean page for an English reader", () => {
    const korean = "지배적인 시퀀스 변환 모델은 복잡한 순환 신경망에 기반한다.";
    expect(needsTranslation(korean, "en")).toBe(true);
  });

  it("does not flag an English page for an English reader", () => {
    const english = "The Transformer is a new network architecture based on attention.";
    expect(needsTranslation(english, "en")).toBe(false);
  });

  it("ignores a page with too little text to judge", () => {
    expect(needsTranslation("Fig. 1", "ko")).toBe(false);
  });

  it("treats mixed Korean and English by majority script", () => {
    // Mostly Korean with a few English terms — a Korean reader does not need it.
    const mixed = "이 논문은 Transformer 구조를 사용하여 attention 메커니즘을 다룬다. 실험 결과는 우수하다.";
    expect(needsTranslation(mixed, "ko")).toBe(false);
  });
});
