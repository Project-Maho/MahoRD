export type ConnectionQuality = "good" | "degraded" | "poor" | "unknown";
export interface QualitySample { rtt_smoothed_ms?: number | null; packet_loss_ratio?: number | null }
export function sustainedQuality(samples: readonly QualitySample[], phase = "streaming"): ConnectionQuality {
  if (phase !== "streaming") return "unknown";
  const recent = samples.slice(-5);
  if (recent.length < 3 || recent.some(s => s.rtt_smoothed_ms == null || s.packet_loss_ratio == null)) return "unknown";
  const poor = recent.filter(s => (s.rtt_smoothed_ms ?? 0) >= 150 || (s.packet_loss_ratio ?? 0) >= 0.05).length;
  const degraded = recent.filter(s => (s.rtt_smoothed_ms ?? 0) >= 60 || (s.packet_loss_ratio ?? 0) >= 0.01).length;
  return poor >= 3 ? "poor" : degraded >= 3 ? "degraded" : "good";
}
