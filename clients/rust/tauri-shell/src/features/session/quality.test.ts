import { expect, test } from "bun:test";
import { sustainedQuality } from "./quality";
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
