export function remoteVirtualKey(event: Pick<KeyboardEvent, "code" | "location" | "keyCode" | "which">): number {
  if (event.code === "NumpadEnter" || (event.keyCode === 13 && event.location === 3)) return 0x100d;
  const numpad = /^Numpad([0-9])$/.exec(event.code ?? "");
  if (numpad) return 0x60 + Number(numpad[1]);
  const functionKey = /^F([0-9]+)$/.exec(event.code ?? "");
  if (functionKey && Number(functionKey[1]) >= 1 && Number(functionKey[1]) <= 24) return 0x6f + Number(functionKey[1]);
  const codes: Record<string, number> = { NumpadDecimal: 0x6e, NumpadDivide: 0x6f, NumpadMultiply: 0x6a, NumpadSubtract: 0x6d, NumpadAdd: 0x6b, NumpadEqual: 0x92, NumLock: 0x90, AudioVolumeMute: 0xad, AudioVolumeDown: 0xae, AudioVolumeUp: 0xaf, MediaTrackNext: 0xb0, MediaTrackPrevious: 0xb1, MediaStop: 0xb2, MediaPlayPause: 0xb3, BrowserBack: 0xa6, BrowserForward: 0xa7, BrowserRefresh: 0xa8, BrowserStop: 0xa9, BrowserSearch: 0xaa, BrowserFavorites: 0xab, BrowserHome: 0xac };
  return codes[event.code] ?? event.keyCode ?? event.which ?? 0;
}
