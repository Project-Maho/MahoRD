export interface RendererHandle {
  render(buf: ArrayBuffer): void;
  dispose(): void;
  readonly backend: "webgl2" | "webgl";
}

export interface ParsedFrame {
  width: number;
  height: number;
  y: Uint8Array;
  /** Byte stride between chroma rows; `width` bytes per NV12 interleaved row. */
  uvStride: number;
  uv: Uint8Array;
  cursor: { x: number; y: number; visible: boolean } | null;
}

const VS_SOURCE_WEBGL2 = `#version 300 es
  in vec2 a_pos;
  in vec2 a_texCoord;
  out vec2 v_texCoord;
  void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_texCoord = a_texCoord;
  }
`;

const FS_SOURCE_WEBGL2 = `#version 300 es
  precision highp float;
  in vec2 v_texCoord;
  out vec4 fragColor;
  uniform sampler2D u_yPlane;
  uniform sampler2D u_uvPlane;
  void main() {
    // Full range: maho-host session.rs bgra_to_nv12 produces full-range BT.601 (JPEG) with no luma offset
    float y = texture(u_yPlane, v_texCoord).r;
    vec2 uv = texture(u_uvPlane, v_texCoord).rg - vec2(0.5, 0.5);
    float u = uv.r;
    float v = uv.g;

    float r = y + 1.402 * v;
    float g = y - 0.344136 * u - 0.714136 * v;
    float b = y + 1.772 * u;
    fragColor = vec4(clamp(vec3(r, g, b), 0.0, 1.0), 1.0);
  }
`;

const VS_SOURCE_WEBGL1 = `
  attribute vec2 a_pos;
  attribute vec2 a_texCoord;
  varying vec2 v_texCoord;
  void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_texCoord = a_texCoord;
  }
`;

const FS_SOURCE_WEBGL1 = `
  precision highp float;
  varying vec2 v_texCoord;
  uniform sampler2D u_yPlane;
  uniform sampler2D u_uvPlane;
  void main() {
    // Full range: maho-host session.rs bgra_to_nv12 produces full-range BT.601 (JPEG) with no luma offset
    float y = texture2D(u_yPlane, v_texCoord).r;
    vec2 uv = texture2D(u_uvPlane, v_texCoord).ra - vec2(0.5, 0.5);
    float u = uv.r;
    float v = uv.g;

    float r = y + 1.402 * v;
    float g = y - 0.344136 * u - 0.714136 * v;
    float b = y + 1.772 * u;
    gl_FragColor = vec4(clamp(vec3(r, g, b), 0.0, 1.0), 1.0);
  }
`;

function compileShader(
  gl: WebGLRenderingContext | WebGL2RenderingContext,
  type: number,
  src: string
): WebGLShader | null {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    console.error("Shader compile error:", gl.getShaderInfoLog(shader));
    gl.deleteShader(shader);
    return null;
  }
  return shader;
}

export function parseFrame(buf: ArrayBuffer): ParsedFrame | null {
  try {
    if (!buf || typeof buf.byteLength !== "number" || buf.byteLength < 16) {
      return null;
    }

    const view = new DataView(buf);
    const width = view.getUint32(0, true);
    const height = view.getUint32(4, true);

    const yLen = width * height;
    const uvHeight = Math.ceil(height / 2);
    // NV12 interleaves one chroma byte pair per 2x2 luma block, so a chroma row
    // spans `width` bytes even when width is odd and the row only carries
    // floor(width / 2) complete UV pairs.
    const uvStride = width;
    const uvLen = uvStride * uvHeight;
    const planesEnd = 16 + yLen + uvLen;

    // The framing must match exactly: a buffer packed with a different plane
    // geometry (e.g. a floor()'d uv height on odd sizes) would otherwise be
    // mis-sliced and its 9-byte cursor tail read as pixel data. Accept only the
    // two valid sizes: planes alone, or planes plus a complete cursor record.
    if (
      !Number.isSafeInteger(planesEnd) ||
      (buf.byteLength !== planesEnd && buf.byteLength !== planesEnd + 9)
    ) {
      return null;
    }

    const y = new Uint8Array(buf, 16, yLen);
    const uv = new Uint8Array(buf, 16 + yLen, uvLen);

    let cursor: { x: number; y: number; visible: boolean } | null = null;
    if (buf.byteLength === planesEnd + 9) {
      const cursorOffset = planesEnd;
      const cursorX = view.getFloat32(cursorOffset, true);
      const cursorY = view.getFloat32(cursorOffset + 4, true);
      const cursorType = view.getUint8(cursorOffset + 8);
      cursor = {
        x: cursorX,
        y: cursorY,
        visible: cursorType > 0,
      };
    }

    return {
      width,
      height,
      y,
      uvStride,
      uv,
      cursor,
    };
  } catch {
    return null;
  }
}

export function createRenderer(canvas: HTMLCanvasElement): RendererHandle | null {
  if (!canvas || typeof canvas.getContext !== "function") {
    return null;
  }

  const glOpts: WebGLContextAttributes = {
    alpha: false,
    depth: false,
    antialias: false,
    preserveDrawingBuffer: false,
    powerPreference: "high-performance",
  };

  let gl: WebGLRenderingContext | WebGL2RenderingContext | null = null;
  let backend: "webgl2" | "webgl" = "webgl2";

  try {
    gl = canvas.getContext("webgl2", glOpts) as WebGL2RenderingContext | null;
    if (gl) {
      backend = "webgl2";
    } else {
      gl = canvas.getContext("webgl", glOpts) as WebGLRenderingContext | null;
      backend = "webgl";
    }
  } catch {
    return null;
  }

  if (!gl) {
    return null;
  }

  const isWebGL2 = backend === "webgl2";

  interface GlResources {
    program: WebGLProgram;
    vs: WebGLShader;
    fs: WebGLShader;
    posBuf: WebGLBuffer;
    texBuf: WebGLBuffer;
    yTexture: WebGLTexture;
    uvTexture: WebGLTexture;
  }

  // Every GL object dies with the context, so creation lives in one function
  // that can be re-run verbatim when a lost context is restored.
  function initResources(
    gl: WebGLRenderingContext | WebGL2RenderingContext
  ): GlResources | null {
    const vs = compileShader(gl, gl.VERTEX_SHADER, isWebGL2 ? VS_SOURCE_WEBGL2 : VS_SOURCE_WEBGL1);
    const fs = compileShader(gl, gl.FRAGMENT_SHADER, isWebGL2 ? FS_SOURCE_WEBGL2 : FS_SOURCE_WEBGL1);
    if (!vs || !fs) {
      if (vs) gl.deleteShader(vs);
      if (fs) gl.deleteShader(fs);
      return null;
    }

    const program = gl.createProgram();
    if (!program) {
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      return null;
    }

    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);

    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      console.error("Program link error:", gl.getProgramInfoLog(program));
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      gl.deleteProgram(program);
      return null;
    }

    gl.useProgram(program);

    const posBuf = gl.createBuffer();
    if (!posBuf) {
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      gl.deleteProgram(program);
      return null;
    }

    gl.bindBuffer(gl.ARRAY_BUFFER, posBuf);
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([
        -1.0, -1.0,  1.0, -1.0, -1.0,  1.0,
        -1.0,  1.0,  1.0, -1.0,  1.0,  1.0,
      ]),
      gl.STATIC_DRAW
    );

    const posLoc = gl.getAttribLocation(program, "a_pos");
    if (posLoc !== -1) {
      gl.enableVertexAttribArray(posLoc);
      gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0);
    }

    const texBuf = gl.createBuffer();
    if (!texBuf) {
      gl.deleteBuffer(posBuf);
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      gl.deleteProgram(program);
      return null;
    }

    gl.bindBuffer(gl.ARRAY_BUFFER, texBuf);
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([
        0.0, 1.0,  1.0, 1.0,  0.0, 0.0,
        0.0, 0.0,  1.0, 1.0,  1.0, 0.0,
      ]),
      gl.STATIC_DRAW
    );

    const texLoc = gl.getAttribLocation(program, "a_texCoord");
    if (texLoc !== -1) {
      gl.enableVertexAttribArray(texLoc);
      gl.vertexAttribPointer(texLoc, 2, gl.FLOAT, false, 0, 0);
    }

    const yTexture = gl.createTexture();
    const uvTexture = gl.createTexture();
    if (!yTexture || !uvTexture) {
      if (yTexture) gl.deleteTexture(yTexture);
      if (uvTexture) gl.deleteTexture(uvTexture);
      gl.deleteBuffer(texBuf);
      gl.deleteBuffer(posBuf);
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      gl.deleteProgram(program);
      return null;
    }

    gl.bindTexture(gl.TEXTURE_2D, yTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);

    gl.bindTexture(gl.TEXTURE_2D, uvTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);

    gl.uniform1i(gl.getUniformLocation(program, "u_yPlane"), 0);
    gl.uniform1i(gl.getUniformLocation(program, "u_uvPlane"), 1);

    return { program, vs, fs, posBuf, texBuf, yTexture, uvTexture };
  }

  let resources = initResources(gl);
  if (!resources) {
    return null;
  }

  let disposed = false;
  let lastRenderWidth = 0;
  let lastRenderHeight = 0;

  const onContextLost = (e: Event) => {
    e.preventDefault();
    // The driver already destroyed every GL object; drop our handles so nothing
    // is used or deleted after the loss.
    resources = null;
    lastRenderWidth = 0;
    lastRenderHeight = 0;
  };
  const onContextRestored = () => {
    if (disposed || !gl) return;
    // Rebuild program, buffers, textures and sampler state; zeroed dimensions
    // force the next frame to reallocate both plane textures.
    resources = initResources(gl);
    lastRenderWidth = 0;
    lastRenderHeight = 0;
  };
  canvas.addEventListener("webglcontextlost", onContextLost, false);
  canvas.addEventListener("webglcontextrestored", onContextRestored, false);

  function renderNv12(
    width: number,
    height: number,
    yData: Uint8Array,
    uvData: Uint8Array,
    uvStride: number
  ): void {
    if (disposed || !gl || !resources || gl.isContextLost() || width <= 0 || height <= 0) return;
    const { yTexture, uvTexture } = resources;

    const uvWidth = Math.floor(width / 2);
    const uvHeight = Math.ceil(height / 2);

    if (lastRenderWidth !== width || lastRenderHeight !== height) {
      canvas.width = width;
      canvas.height = height;
      gl.viewport(0, 0, width, height);
      lastRenderWidth = width;
      lastRenderHeight = height;

      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, yTexture);
      if (isWebGL2) {
        const gl2 = gl as WebGL2RenderingContext;
        gl2.texImage2D(gl2.TEXTURE_2D, 0, gl2.R8, width, height, 0, gl2.RED, gl2.UNSIGNED_BYTE, null);
      } else {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.LUMINANCE, width, height, 0, gl.LUMINANCE, gl.UNSIGNED_BYTE, null);
      }

      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, uvTexture);
      if (isWebGL2) {
        const gl2 = gl as WebGL2RenderingContext;
        gl2.texImage2D(gl2.TEXTURE_2D, 0, gl2.RG8, uvWidth, uvHeight, 0, gl2.RG, gl2.UNSIGNED_BYTE, null);
      } else {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.LUMINANCE_ALPHA, uvWidth, uvHeight, 0, gl.LUMINANCE_ALPHA, gl.UNSIGNED_BYTE, null);
      }
    }

    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, yTexture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    if (isWebGL2) {
      const gl2 = gl as WebGL2RenderingContext;
      gl2.texSubImage2D(gl2.TEXTURE_2D, 0, 0, 0, width, height, gl2.RED, gl2.UNSIGNED_BYTE, yData);
    } else {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, width, height, gl.LUMINANCE, gl.UNSIGNED_BYTE, yData);
    }

    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, uvTexture);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    const uvFormat = isWebGL2 ? (gl as WebGL2RenderingContext).RG : gl.LUMINANCE_ALPHA;
    // The chroma texture is floor(width / 2) RG texels (i.e. `uvRowBytes`) wide,
    // but a chroma row is strided by `uvStride` bytes. The two differ on odd
    // widths, so a tightly packed upload would shear every row.
    const uvRowBytes = uvWidth * 2;
    if (uvStride <= uvRowBytes) {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, uvWidth, uvHeight, uvFormat, gl.UNSIGNED_BYTE, uvData);
    } else if (isWebGL2 && uvStride % 2 === 0) {
      const gl2 = gl as WebGL2RenderingContext;
      gl2.pixelStorei(gl2.UNPACK_ROW_LENGTH, uvStride / 2);
      gl2.texSubImage2D(gl2.TEXTURE_2D, 0, 0, 0, uvWidth, uvHeight, uvFormat, gl2.UNSIGNED_BYTE, uvData);
      gl2.pixelStorei(gl2.UNPACK_ROW_LENGTH, 0);
    } else {
      // An odd byte stride cannot be expressed through UNPACK_ROW_LENGTH
      // (which counts texels), so upload one row at a time.
      for (let row = 0; row < uvHeight; row++) {
        const start = row * uvStride;
        gl.texSubImage2D(
          gl.TEXTURE_2D,
          0,
          0,
          row,
          uvWidth,
          1,
          uvFormat,
          gl.UNSIGNED_BYTE,
          uvData.subarray(start, start + uvRowBytes)
        );
      }
    }

    gl.drawArrays(gl.TRIANGLES, 0, 6);
  }

  function cleanup(): void {
    canvas.removeEventListener("webglcontextlost", onContextLost, false);
    canvas.removeEventListener("webglcontextrestored", onContextRestored, false);
    if (!gl || !resources) return;
    const { program, vs, fs, yTexture, uvTexture, posBuf, texBuf } = resources;
    resources = null;
    try {
      gl.deleteTexture(yTexture);
      gl.deleteTexture(uvTexture);
      gl.deleteBuffer(posBuf);
      gl.deleteBuffer(texBuf);
      if (vs) {
        gl.detachShader(program, vs);
        gl.deleteShader(vs);
      }
      if (fs) {
        gl.detachShader(program, fs);
        gl.deleteShader(fs);
      }
      gl.deleteProgram(program);
    } catch {
      // Ignore errors on already destroyed GL context
    }
  }

  return {
    backend,
    render(buf: ArrayBuffer): void {
      if (disposed) return;
      const frame = parseFrame(buf);
      if (!frame) return;
      renderNv12(frame.width, frame.height, frame.y, frame.uv, frame.uvStride);
    },
    dispose(): void {
      if (disposed) return;
      disposed = true;
      cleanup();
    },
  };
}
