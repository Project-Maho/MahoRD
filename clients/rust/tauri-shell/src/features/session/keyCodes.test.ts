import { expect, test } from "bun:test";
import { remoteVirtualKey } from "./keyCodes";
test("physical numpad identity survives NumLock and extended function codes", () => {
  const key = (code: string, keyCode: number, location = 0) => remoteVirtualKey({ code, keyCode, which: keyCode, location });
  expect(key("NumpadEnter", 13, 3)).toBe(0x100d);
  expect(key("Enter", 13)).toBe(13);
  expect(key("Numpad0", 45, 3)).toBe(0x60);
  expect(key("F20", 0)).toBe(0x83);
  expect(key("AudioVolumeUp", 0)).toBe(0xaf);
  for (let n = 21; n <= 24; n++) expect(key(`F${n}`, 0)).toBe(0x6f + n);
  for (const [code, vk] of [["MediaPlayPause", 0xb3], ["MediaTrackNext", 0xb0], ["MediaTrackPrevious", 0xb1], ["MediaStop", 0xb2], ["BrowserBack", 0xa6], ["BrowserForward", 0xa7], ["BrowserRefresh", 0xa8], ["BrowserStop", 0xa9], ["BrowserSearch", 0xaa], ["BrowserFavorites", 0xab], ["BrowserHome", 0xac]] as const) expect(key(code, 0)).toBe(vk);
});
