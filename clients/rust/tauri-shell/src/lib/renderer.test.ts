import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { createRenderer, parseFrame } from "./renderer";

function createFrameBuffer(
  width: number,
  height: number,
  tail?: { x: number; y: number; type: number } | number[]
): ArrayBuffer {
  const yLen = width * height;
  // NV12 chroma rows are strided by the full luma width (one chroma byte per
  // luma column), exactly as the Rust IPC packer writes them.
  const uvHeight = Math.ceil(height / 2);
  const uvLen = width * uvHeight;
  const tailLen = tail ? (Array.isArray(tail) ? tail.length : 9) : 0;
  const totalLen = 16 + yLen + uvLen + tailLen;

  const buf = new ArrayBuffer(totalLen);
  const view = new DataView(buf);
  view.setUint32(0, width, true);
  view.setUint32(4, height, true);

  const yData = new Uint8Array(buf, 16, yLen);
  for (let i = 0; i < yLen; i++) {
    yData[i] = (i & 0xff);
  }

  const uvData = new Uint8Array(buf, 16 + yLen, uvLen);
  for (let i = 0; i < uvLen; i++) {
    uvData[i] = ((i * 2) & 0xff);
  }

  if (tail) {
    const tailOffset = 16 + yLen + uvLen;
    if (Array.isArray(tail)) {
      const u8 = new Uint8Array(buf);
      for (let i = 0; i < tail.length; i++) {
        u8[tailOffset + i] = tail[i];
      }
    } else {
      view.setFloat32(tailOffset, tail.x, true);
      view.setFloat32(tailOffset + 4, tail.y, true);
      view.setUint8(tailOffset + 8, tail.type);
    }
  }

  return buf;
}

describe("parseFrame", () => {
  it("a well-formed buffer yields the right width, height, plane lengths and a null cursor", () => {
    const width = 64;
    const height = 48;
    const yLen = 64 * 48;
    const uvLen = 64 * Math.ceil(48 / 2);
    const buf = createFrameBuffer(width, height);

    const parsed = parseFrame(buf);
    expect(parsed).not.toBeNull();
    expect(parsed?.width).toBe(width);
    expect(parsed?.height).toBe(height);
    expect(parsed?.y.byteLength).toBe(yLen);
    expect(parsed?.uv.byteLength).toBe(uvLen);
    expect(parsed?.uvStride).toBe(width);
    expect(parsed?.cursor).toBeNull();
  });

  it("the same buffer plus a 9-byte tail yields the cursor x, y and visible flag", () => {
    const width = 64;
    const height = 48;
    const buf = createFrameBuffer(width, height, { x: 142.25, y: 284.5, type: 1 });

    const parsed = parseFrame(buf);
    expect(parsed).not.toBeNull();
    expect(parsed?.width).toBe(width);
    expect(parsed?.height).toBe(height);
    expect(parsed?.cursor).not.toBeNull();
    expect(parsed?.cursor?.x).toBeCloseTo(142.25, 2);
    expect(parsed?.cursor?.y).toBeCloseTo(284.5, 2);
    expect(parsed?.cursor?.visible).toBe(true);
  });

  it("cursor type 0 yields visible false", () => {
    const width = 32;
    const height = 32;
    const buf = createFrameBuffer(width, height, { x: 10.0, y: 20.0, type: 0 });

    const parsed = parseFrame(buf);
    expect(parsed).not.toBeNull();
    expect(parsed?.cursor).not.toBeNull();
    expect(parsed?.cursor?.visible).toBe(false);
    expect(parsed?.cursor?.x).toBeCloseTo(10.0, 2);
    expect(parsed?.cursor?.y).toBeCloseTo(20.0, 2);
  });

  it("cursor type > 0 (e.g. type 2) yields visible true", () => {
    const width = 16;
    const height = 16;
    const buf = createFrameBuffer(width, height, { x: 5.5, y: 7.5, type: 2 });

    const parsed = parseFrame(buf);
    expect(parsed).not.toBeNull();
    expect(parsed?.cursor).not.toBeNull();
    expect(parsed?.cursor?.visible).toBe(true);
  });

  it("buffers shorter than the header yield null instead of throwing", () => {
    expect(parseFrame(new ArrayBuffer(0))).toBeNull();
    expect(parseFrame(new ArrayBuffer(4))).toBeNull();
    expect(parseFrame(new ArrayBuffer(15))).toBeNull();
  });

  it("buffers shorter than header+planes both yield null instead of throwing", () => {
    const width = 64;
    const height = 48;
    const fullBuf = createFrameBuffer(width, height);
    // fullBuf length is 16 + 3072 + 1536 = 4624 bytes
    const headerOnly = fullBuf.slice(0, 16);
    const partialY = fullBuf.slice(0, 16 + 100);
    const yOnly = fullBuf.slice(0, 16 + 3072);
    const partialUv = fullBuf.slice(0, fullBuf.byteLength - 1);

    expect(parseFrame(headerOnly)).toBeNull();
    expect(parseFrame(partialY)).toBeNull();
    expect(parseFrame(yOnly)).toBeNull();
    expect(parseFrame(partialUv)).toBeNull();
  });

  it("buffers whose size does not match the framing exactly are rejected", () => {
    const width = 16;
    const height = 16;
    // 8 extra bytes instead of the 9-byte cursor record: the framing does not
    // match, so the buffer is rejected rather than silently mis-sliced.
    const truncatedTail = createFrameBuffer(width, height, [1, 2, 3, 4, 5, 6, 7, 8]);
    expect(parseFrame(truncatedTail)).toBeNull();

    // Extra bytes beyond the cursor record are equally a framing mismatch.
    const overlongTail = createFrameBuffer(
      width,
      height,
      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    );
    expect(parseFrame(overlongTail)).toBeNull();
  });

  it("an odd-height frame packed with a floor()'d uv plane is rejected, not mis-sliced", () => {
    // A producer that truncates uvHeight with floor packs fewer UV bytes than
    // this parser's ceil geometry requires. The two disagree, so the 9-byte
    // cursor tail would land inside the UV plane if the length check were loose.
    const width = 16;
    const height = 15;
    const yLen = width * height;
    const flooredUvLen = width * Math.floor(height / 2); // 7 rows
    const ceilUvLen = width * Math.ceil(height / 2); // 8 rows
    expect(flooredUvLen).not.toBe(ceilUvLen);

    const buf = new ArrayBuffer(16 + yLen + flooredUvLen + 9);
    const view = new DataView(buf);
    view.setUint32(0, width, true);
    view.setUint32(4, height, true);

    expect(parseFrame(buf)).toBeNull();
  });

  it("calculates UV dimensions matching NV12 half-sampling math", () => {
    const width = 65;
    const height = 47;
    const yLen = 65 * 47;
    // Chroma rows are strided by the luma width even on an odd width, so the
    // plane is width * ceil(height / 2) bytes, not a tightly packed half-width.
    const uvLen = 65 * Math.ceil(47 / 2); // 65 * 24
    const buf = createFrameBuffer(width, height);

    const parsed = parseFrame(buf);
    expect(parsed).not.toBeNull();
    expect(parsed?.width).toBe(65);
    expect(parsed?.height).toBe(47);
    expect(parsed?.y.byteLength).toBe(yLen);
    expect(parsed?.uv.byteLength).toBe(uvLen);
    expect(parsed?.uvStride).toBe(65);
  });
});

interface Nv12Pixel {
  y: number;
  u: number;
  v: number;
}

interface RgbPixel {
  r: number;
  g: number;
  b: number;
}

function clamp(val: number, min: number, max: number): number {
  return Math.min(Math.max(val, min), max);
}

function hostBgraToNv12(r: number, g: number, b: number): Nv12Pixel {
  const y = (77 * r + 150 * g + 29 * b) >> 8;
  const u = 128 + ((-43 * r - 85 * g + 128 * b) >> 8);
  const v = 128 + ((128 * r - 107 * g - 21 * b) >> 8);
  return { y, u, v };
}

// ---------------------------------------------------------------------------
// Round-trip math derived from the ACTUAL GLSL in renderer.ts.
// The fragment shader source is parsed below; any regression that mutates the
// matrix coefficients, swaps the U/V channels or offsets, or changes the
// shader shape makes extraction fail or the round-trip colors drift, failing
// the whole "BT.601 full-range NV12 round-trip" suite.
// ---------------------------------------------------------------------------
interface ShaderMath {
  uChannel: string;
  vChannel: string;
  uOffset: number;
  vOffset: number;
  rExpr: string;
  gExpr: string;
  bExpr: string;
}

function extractShaderMath(src: string): ShaderMath {
  const fail = (what: string): never => {
    throw new Error(
      `renderer.ts fragment shader changed shape and could not be parsed (${what}); fix the shader regression or update extractShaderMath`
    );
  };
  const capture = (re: RegExp, what: string): RegExpMatchArray => {
    const m = src.match(re);
    if (!m) return fail(what);
    return m;
  };
  const exactly = (text: string, count: number, what: string): void => {
    const found = src.split(text).length - 1;
    if (found !== count) {
      fail(`${what} appears ${found} time(s), expected ${count} (both shader backends must agree)`);
    }
  };
  const uvLine = capture(
    /vec2 uv = texture2D\(u_uvPlane, v_texCoord\)\.([a-z]+) - vec2\(([\d.]+), ([\d.]+)\)/,
    "uv sampling line (WebGL1)"
  );
  const uAssign = capture(/float u = uv\.([a-z]+);/, "u channel assignment");
  const vAssign = capture(/float v = uv\.([a-z]+);/, "v channel assignment");
  const rLine = capture(/float r = ([^;]+);/, "r channel expression");
  const gLine = capture(/float g = ([^;]+);/, "g channel expression");
  const bLine = capture(/float b = ([^;]+);/, "b channel expression");
  // Pin the channel topology: NV12 packs U first, V second; WebGL2's RG
  // texture carries U in .r and V in .g, WebGL1's LUMINANCE_ALPHA carries U
  // in .r and V in .a. Any U/V swap changes one of these exact strings.
  exactly(
    `vec2 uv = texture2D(u_uvPlane, v_texCoord).${uvLine[1]} - vec2(${uvLine[2]}, ${uvLine[3]});`,
    1,
    "WebGL1 uv swizzle"
  );
  exactly(
    `vec2 uv = texture(u_uvPlane, v_texCoord).rg - vec2(${uvLine[2]}, ${uvLine[3]});`,
    1,
    "WebGL2 uv swizzle"
  );
  exactly(`float u = uv.${uAssign[1]};`, 2, "u assignment");
  exactly(`float v = uv.${vAssign[1]};`, 2, "v assignment");
  exactly(`float r = ${rLine[1]};`, 2, "r expression");
  exactly(`float g = ${gLine[1]};`, 2, "g expression");
  exactly(`float b = ${bLine[1]};`, 2, "b expression");
  return {
    uChannel: uAssign[1],
    vChannel: vAssign[1],
    uOffset: Number(uvLine[2]),
    vOffset: Number(uvLine[3]),
    rExpr: rLine[1],
    gExpr: gLine[1],
    bExpr: bLine[1],
  };
}

const rendererSource = readFileSync(new URL("./renderer.ts", import.meta.url), "utf8");
const shaderMath = extractShaderMath(rendererSource);

/** Evaluates a simple GLSL scalar expression over the y/u/v variables. */
function evalShaderExpr(expr: string, y: number, u: number, v: number): number {
  if (!/^[yuv0-9.()\s+\-*/]+$/.test(expr)) {
    throw new Error(`shader expression uses unsupported syntax: "${expr}"`);
  }
  const fn = new Function("y", "u", "v", `"use strict"; return (${expr});`) as (
    y: number,
    u: number,
    v: number
  ) => number;
  const out = fn(y, u, v);
  if (typeof out !== "number" || !Number.isFinite(out)) {
    throw new Error(`shader expression did not evaluate to a finite number: "${expr}"`);
  }
  return out;
}

function shaderNv12ToRgb(y: number, u: number, v: number): RgbPixel {
  // y/u/v are the raw NV12 bytes exactly as the host packs them. The UV plane
  // interleaves one U byte followed by one V byte: texture channel 'r' always
  // carries U, and the shader's v assignment selects the channel carrying V
  // ('g' for the WebGL2 RG texture, 'a' for the WebGL1 LUMINANCE_ALPHA one).
  const sample = (channel: string, uByte: number, vByte: number): number => {
    if (channel === "r") return uByte / 255.0;
    if (channel === "g" || channel === "a") return vByte / 255.0;
    throw new Error(`unexpected chroma texture channel: .${channel}`);
  };
  // Texture sampling normalizes unsigned bytes [0, 255] to [0.0, 1.0]
  const yNorm = y / 255.0;
  const uNorm = sample(shaderMath.uChannel, u, v) - shaderMath.uOffset;
  const vNorm = sample(shaderMath.vChannel, u, v) - shaderMath.vOffset;

  // The shader's own r/g/b expressions (clamped by the shader's vec4 clamp).
  const to255 = (norm: number): number => clamp(norm, 0.0, 1.0) * 255.0;
  return {
    r: to255(evalShaderExpr(shaderMath.rExpr, yNorm, uNorm, vNorm)),
    g: to255(evalShaderExpr(shaderMath.gExpr, yNorm, uNorm, vNorm)),
    b: to255(evalShaderExpr(shaderMath.bExpr, yNorm, uNorm, vNorm)),
  };
}

describe("shader artifact range validation", () => {
  it("fails if limited-range BT.601 shader expansions or offsets are reintroduced in renderer.ts", async () => {
    const rendererSrc = await Bun.file(new URL("./renderer.ts", import.meta.url)).text();

    expect(rendererSrc.includes("255.0 / 219.0")).toBe(false);
    expect(rendererSrc.includes("255.0/219.0")).toBe(false);
    expect(rendererSrc.includes("255.0 / 224.0")).toBe(false);
    expect(rendererSrc.includes("255.0/224.0")).toBe(false);
    expect(rendererSrc.includes("16.0 / 255.0")).toBe(false);
    expect(rendererSrc.includes("16.0/255.0")).toBe(false);

    expect(rendererSrc.includes("texture(u_yPlane")).toBe(true);
    expect(rendererSrc.includes("texture2D(u_yPlane")).toBe(true);

    const occurrences = rendererSrc.split("- vec2(0.5, 0.5)").length - 1;
    expect(occurrences).toBeGreaterThanOrEqual(2);
  });
});

describe("BT.601 full-range NV12 round-trip", () => {
  // Tolerance of 3/255 (~0.01176 normalized, or 3.0 in 0..255 space)
  // Max observed integer-quantization error across test colors is ~1.07/255.
  const TOLERANCE_255 = 3;
  const TOLERANCE_NORM = 3 / 255;

  const testCases: { name: string; r: number; g: number; b: number }[] = [
    { name: "black", r: 0, g: 0, b: 0 },
    { name: "white", r: 255, g: 255, b: 255 },
    { name: "mid grey", r: 128, g: 128, b: 128 },
    { name: "pure red", r: 255, g: 0, b: 0 },
    { name: "pure green", r: 0, g: 255, b: 0 },
    { name: "pure blue", r: 0, g: 0, b: 255 },
  ];

  for (const { name, r, g, b } of testCases) {
    it(`round-trips ${name} (r=${r}, g=${g}, b=${b}) within 3/255 tolerance`, () => {
      const { y, u, v } = hostBgraToNv12(r, g, b);
      const out = shaderNv12ToRgb(y, u, v);

      const errR = Math.abs(out.r - r);
      const errG = Math.abs(out.g - g);
      const errB = Math.abs(out.b - b);

      expect(errR).toBeLessThanOrEqual(TOLERANCE_255);
      expect(errG).toBeLessThanOrEqual(TOLERANCE_255);
      expect(errB).toBeLessThanOrEqual(TOLERANCE_255);

      expect(errR / 255).toBeLessThanOrEqual(TOLERANCE_NORM);
      expect(errG / 255).toBeLessThanOrEqual(TOLERANCE_NORM);
      expect(errB / 255).toBeLessThanOrEqual(TOLERANCE_NORM);
    });
  }

  it("discriminates against limited-range decoding with a tight <= 1/255 bound on mid grey", () => {
    // Under the old limited-range math:
    //   y = (128.0 / 255.0 - 16.0 / 255.0) * (255.0 / 219.0) = 112 / 219 = 0.5114155
    //   which produced 130.41, i.e. an error of 2.41/255 from 128.
    // Under full-range math:
    //   y = 128.0 / 255.0, u = 128.0 / 255.0 - 0.5, v = 128.0 / 255.0 - 0.5
    //   which round-trips 128 exactly (with chroma offset 0.5/255 yielding max error ~0.886/255).
    // An error bound of <= 1/255 strictly passes for full-range math and fails for limited-range math,
    // so this assertion is what actually fails if the limited-range bug returns.
    const TIGHT_TOLERANCE_255 = 1;
    const TIGHT_TOLERANCE_NORM = 1 / 255;

    const { y, u, v } = hostBgraToNv12(128, 128, 128);
    const out = shaderNv12ToRgb(y, u, v);

    const errR = Math.abs(out.r - 128);
    const errG = Math.abs(out.g - 128);
    const errB = Math.abs(out.b - 128);

    expect(errR).toBeLessThanOrEqual(TIGHT_TOLERANCE_255);
    expect(errG).toBeLessThanOrEqual(TIGHT_TOLERANCE_255);
    expect(errB).toBeLessThanOrEqual(TIGHT_TOLERANCE_255);

    expect(errR / 255).toBeLessThanOrEqual(TIGHT_TOLERANCE_NORM);
    expect(errG / 255).toBeLessThanOrEqual(TIGHT_TOLERANCE_NORM);
    expect(errB / 255).toBeLessThanOrEqual(TIGHT_TOLERANCE_NORM);
  });

  it("explicitly asserts that WHITE comes back near 255 without being clipped by limited-range math", () => {
    const { y, u, v } = hostBgraToNv12(255, 255, 255);
    // Luma for full-range white spans up to 255 (not 235)
    expect(y).toBe(255);
    const out = shaderNv12ToRgb(y, u, v);
    // Under limited-range math, raw Y was ~278.3 (blown highlights).
    // Under full-range math, RGB values are all within TOLERANCE_255 of 255.
    expect(out.r).toBeGreaterThanOrEqual(255 - TOLERANCE_255);
    expect(out.g).toBeGreaterThanOrEqual(255 - TOLERANCE_255);
    expect(out.b).toBeGreaterThanOrEqual(255 - TOLERANCE_255);
    expect(Math.abs(out.r - 255)).toBeLessThanOrEqual(TOLERANCE_255);
    expect(Math.abs(out.g - 255)).toBeLessThanOrEqual(TOLERANCE_255);
    expect(Math.abs(out.b - 255)).toBeLessThanOrEqual(TOLERANCE_255);
  });

  it("explicitly asserts that BLACK comes back near 0 without being crushed by limited-range math", () => {
    const { y, u, v } = hostBgraToNv12(0, 0, 0);
    // Luma for full-range black starts at 0 (not 16)
    expect(y).toBe(0);
    const out = shaderNv12ToRgb(y, u, v);
    // Under limited-range math, raw Y was -18.6 (crushed blacks below 16).
    // Under full-range math, RGB values are all within TOLERANCE_255 of 0.
    expect(out.r).toBeLessThanOrEqual(TOLERANCE_255);
    expect(out.g).toBeLessThanOrEqual(TOLERANCE_255);
    expect(out.b).toBeLessThanOrEqual(TOLERANCE_255);
    expect(Math.abs(out.r - 0)).toBeLessThanOrEqual(TOLERANCE_255);
    expect(Math.abs(out.g - 0)).toBeLessThanOrEqual(TOLERANCE_255);
    expect(Math.abs(out.b - 0)).toBeLessThanOrEqual(TOLERANCE_255);
  });
});

// Minimal WebGL2 double: records the calls the renderer makes so context-loss
// recovery and UV row-stride handling can be asserted without a real GPU.
interface FakeGlCall {
  name: string;
  args: unknown[];
}

function createFakeCanvas() {
  const calls: FakeGlCall[] = [];
  const listeners: Record<string, ((e: any) => void)[]> = {};
  let contextLost = false;
  let nextId = 1;

  const record =
    (name: string, result?: unknown) =>
    (...args: unknown[]) => {
      calls.push({ name, args });
      return result;
    };

  const gl: any = {
    VERTEX_SHADER: 0x8b31,
    FRAGMENT_SHADER: 0x8b30,
    COMPILE_STATUS: 0x8b81,
    LINK_STATUS: 0x8b82,
    ARRAY_BUFFER: 0x8892,
    STATIC_DRAW: 0x88e4,
    FLOAT: 0x1406,
    TEXTURE_2D: 0x0de1,
    TEXTURE0: 0x84c0,
    TEXTURE1: 0x84c1,
    TEXTURE_WRAP_S: 0x2802,
    TEXTURE_WRAP_T: 0x2803,
    TEXTURE_MIN_FILTER: 0x2801,
    TEXTURE_MAG_FILTER: 0x2800,
    CLAMP_TO_EDGE: 0x812f,
    LINEAR: 0x2601,
    UNPACK_ALIGNMENT: 0x0cf5,
    UNPACK_ROW_LENGTH: 0x0cf2,
    UNSIGNED_BYTE: 0x1401,
    TRIANGLES: 0x0004,
    RED: 0x1903,
    RG: 0x8227,
    R8: 0x8229,
    RG8: 0x822b,
    LUMINANCE: 0x1909,
    LUMINANCE_ALPHA: 0x190a,
    createShader: record("createShader", { shader: nextId++ }),
    shaderSource: record("shaderSource"),
    compileShader: record("compileShader"),
    getShaderParameter: () => true,
    getShaderInfoLog: () => "",
    deleteShader: record("deleteShader"),
    createProgram: record("createProgram", { program: nextId++ }),
    attachShader: record("attachShader"),
    detachShader: record("detachShader"),
    linkProgram: record("linkProgram"),
    getProgramParameter: () => true,
    getProgramInfoLog: () => "",
    deleteProgram: record("deleteProgram"),
    useProgram: record("useProgram"),
    createBuffer: () => ({ buffer: nextId++ }),
    bindBuffer: record("bindBuffer"),
    bufferData: record("bufferData"),
    deleteBuffer: record("deleteBuffer"),
    getAttribLocation: () => 0,
    enableVertexAttribArray: record("enableVertexAttribArray"),
    vertexAttribPointer: record("vertexAttribPointer"),
    createTexture: () => ({ texture: nextId++ }),
    bindTexture: record("bindTexture"),
    deleteTexture: record("deleteTexture"),
    texParameteri: record("texParameteri"),
    getUniformLocation: () => ({ uniform: nextId++ }),
    uniform1i: record("uniform1i"),
    activeTexture: record("activeTexture"),
    pixelStorei: record("pixelStorei"),
    texImage2D: record("texImage2D"),
    texSubImage2D: record("texSubImage2D"),
    viewport: record("viewport"),
    drawArrays: record("drawArrays"),
    isContextLost: () => contextLost,
  };

  const canvas: any = {
    width: 0,
    height: 0,
    getContext: (kind: string) => (kind === "webgl2" ? gl : null),
    addEventListener(type: string, fn: (e: any) => void) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type: string, fn: (e: any) => void) {
      listeners[type] = (listeners[type] || []).filter((l) => l !== fn);
    },
  };

  return {
    canvas: canvas as HTMLCanvasElement,
    calls,
    loseContext() {
      contextLost = true;
      let prevented = false;
      for (const fn of listeners["webglcontextlost"] || []) {
        fn({ preventDefault: () => (prevented = true) });
      }
      return prevented;
    },
    restoreContext() {
      contextLost = false;
      for (const fn of listeners["webglcontextrestored"] || []) fn({});
    },
  };
}

function nv12Buffer(width: number, height: number): ArrayBuffer {
  const yLen = width * height;
  const uvLen = width * Math.ceil(height / 2);
  const buf = new ArrayBuffer(16 + yLen + uvLen);
  const view = new DataView(buf);
  view.setUint32(0, width, true);
  view.setUint32(4, height, true);
  return buf;
}

describe("WebGL context loss and restore", () => {
  it("rebuilds the program, textures and state after webglcontextrestored", () => {
    const fake = createFakeCanvas();
    const renderer = createRenderer(fake.canvas);
    expect(renderer).not.toBeNull();

    renderer!.render(nv12Buffer(64, 48));
    const drawsBeforeLoss = fake.calls.filter((c) => c.name === "drawArrays").length;
    expect(drawsBeforeLoss).toBe(1);

    const programsBeforeLoss = fake.calls.filter((c) => c.name === "createProgram").length;
    expect(programsBeforeLoss).toBe(1);

    // Context loss must be preventDefault()'d, and nothing may be drawn while lost.
    expect(fake.loseContext()).toBe(true);
    renderer!.render(nv12Buffer(64, 48));
    expect(fake.calls.filter((c) => c.name === "drawArrays").length).toBe(drawsBeforeLoss);

    // Restore rebuilds GL objects and rendering resumes.
    fake.restoreContext();
    expect(fake.calls.filter((c) => c.name === "createProgram").length).toBe(2);
    expect(fake.calls.filter((c) => c.name === "texParameteri").length).toBe(16);

    renderer!.render(nv12Buffer(64, 48));
    expect(fake.calls.filter((c) => c.name === "drawArrays").length).toBe(drawsBeforeLoss + 1);
    // Both plane textures are reallocated for the first frame after restore.
    expect(fake.calls.filter((c) => c.name === "texImage2D").length).toBe(4);

    renderer!.dispose();
  });

  it("does not assume tightly packed UV rows for an odd width", () => {
    const fake = createFakeCanvas();
    const renderer = createRenderer(fake.canvas);
    expect(renderer).not.toBeNull();

    // Odd width: uv row is floor(65 / 2) * 2 = 64 bytes wide in texels,
    // while NV12 chroma rows are strided by the full luma width.
    renderer!.render(nv12Buffer(65, 48));

    const rowLengthCalls = fake.calls.filter(
      (c) => c.name === "pixelStorei" && c.args[0] === 0x0cf2
    );
    const uvUploads = fake.calls.filter(
      (c) => c.name === "texSubImage2D" && c.args[6] === 0x8227
    );
    // Either UNPACK_ROW_LENGTH is set for the strided upload, or rows are
    // uploaded individually; a single tight full-plane upload would shear.
    const strideHandled = rowLengthCalls.length > 0 || uvUploads.length === Math.ceil(48 / 2);
    expect(strideHandled).toBe(true);

    renderer!.dispose();
  });
});

describe("renderer hot path", () => {
  it("render accepts a pre-parsed frame so the buffer is parsed once per frame", () => {
    const fake = createFakeCanvas();
    const renderer = createRenderer(fake.canvas);
    expect(renderer).not.toBeNull();

    const frame = parseFrame(nv12Buffer(64, 48));
    expect(frame).not.toBeNull();
    renderer!.render(frame!);

    expect(fake.calls.filter((c) => c.name === "drawArrays").length).toBe(1);

    renderer!.dispose();
  });
});
