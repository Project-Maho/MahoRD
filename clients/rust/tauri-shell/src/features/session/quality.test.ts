import { expect, test } from "bun:test";
import { nextQualitySamples, sustainedQuality } from "./quality";
test("quality requires sustained complete telemetry", () => {
  const good = { rtt_smoothed_ms: 20, packet_loss_ratio: 0 };
  const poor = { rtt_smoothed_ms: 200, packet_loss_ratio: 0.1 };
  expect(sustainedQuality([good, good])).toBe("unknown");
  expect(sustainedQuality([good, good, poor])).toBe("good");
  expect(sustainedQuality([poor, poor, poor])).toBe("poor");
  expect(sustainedQuality([good, good, {}])).toBe("unknown");
  expect(sustainedQuality(Array(3).fill({ rtt_smoothed_ms: 80, packet_loss_ratio: 0 }))).toBe("degraded");
});
test("non-streaming phases never retain a good quality badge", () => {
  const samples = Array(5).fill({ rtt_smoothed_ms: 20, packet_loss_ratio: 0 });
  for (const phase of ["connecting", "waiting-video", "reconnecting", "gave-up", "ended", "idle"]) {
    expect(sustainedQuality(samples, phase)).toBe("unknown");
  }
  expect(sustainedQuality(samples, "streaming")).toBe("good");
});
test("samples reset when the phase leaves streaming so reconnect starts fresh", () => {
  const poor = { rtt_smoothed_ms: 200, packet_loss_ratio: 0.1 };
  const good = { rtt_smoothed_ms: 20, packet_loss_ratio: 0 };
  let samples = [poor, poor, poor, poor, poor].reduce((acc, s) => nextQualitySamples(acc, "streaming", s), [] as ReturnType<typeof nextQualitySamples>);
  expect(samples).toHaveLength(5);
  expect(sustainedQuality(samples, "streaming")).toBe("poor");
  samples = nextQualitySamples(samples, "reconnecting", null);
  expect(samples).toEqual([]);
  expect(nextQualitySamples(samples, "waiting-video", poor)).toEqual([]);
  for (const s of [good, good, good]) samples = nextQualitySamples(samples, "streaming", s);
  expect(sustainedQuality(samples, "streaming")).toBe("good");
  // A phase-only update while streaming keeps the window unchanged.
  expect(nextQualitySamples(samples, "streaming", null)).toBe(samples);
});
