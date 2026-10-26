import { useEffect, useRef, type CSSProperties } from "react";
import fragmentSource from "@/field.frag?raw";
import { cn } from "@/lib/utils";

export type FieldKind = "hero" | "cta";

type Palette = readonly [string, string, string, string, string];

const PALETTES: Record<FieldKind, { light: Palette; dark: Palette }> = {
  hero: {
    light: ["#EAF0FC", "#D3E0F8", "#AFC6F1", "#7FA3E6", "#1F5FD1"],
    dark: ["#151E2E", "#1A2943", "#22385F", "#2D4C84", "#74ADFF"],
  },
  cta: {
    light: ["#2766D4", "#326ED8", "#4480DE", "#5C92E5", "#FFFFFF"],
    dark: ["#1F4379", "#254B85", "#2E5896", "#3B69AB", "#A9CBFF"],
  },
};

const MODES: Record<FieldKind, number> = { hero: 0, cta: 1 };
const PALETTE_UNIFORMS = ["c1", "c2", "c3", "c4", "hi"] as const;
const VERTEX_SOURCE = "attribute vec2 a;void main(){gl_Position=vec4(a,0.,1.);}";
const TRIANGLE = new Float32Array([-1, -1, 3, -1, -1, 3]);
const TRIANGLE_VERTICES = 3;
const POSITION_COMPONENTS = 2;
const CELL = 3;
const FRAME_INTERVAL_MS = 33;
const START_JITTER_MS = 60000;
const STILL_TIME_MS = 20000;
const MS_PER_SECOND = 1000;
const HEX_CHANNELS = [1, 3, 5];
const HEX_CHANNEL_LENGTH = 2;
const HEX_RADIX = 16;
const CHANNEL_MAX = 255;
const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";

const rgb = (hex: string) =>
  HEX_CHANNELS.map(
    (start) => parseInt(hex.slice(start, start + HEX_CHANNEL_LENGTH), HEX_RADIX) / CHANNEL_MAX,
  );

function compile(gl: WebGLRenderingContext, type: number, source: string) {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  return shader;
}

function program(gl: WebGLRenderingContext) {
  const vertex = compile(gl, gl.VERTEX_SHADER, VERTEX_SOURCE);
  const fragment = compile(gl, gl.FRAGMENT_SHADER, fragmentSource);
  const linked = gl.createProgram();
  if (!vertex || !fragment || !linked) return null;
  gl.attachShader(linked, vertex);
  gl.attachShader(linked, fragment);
  gl.linkProgram(linked);
  if (!gl.getProgramParameter(linked, gl.LINK_STATUS)) return null;
  gl.useProgram(linked);
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ARRAY_BUFFER, TRIANGLE, gl.STATIC_DRAW);
  const position = gl.getAttribLocation(linked, "a");
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, POSITION_COMPONENTS, gl.FLOAT, false, 0, 0);
  return linked;
}

export function Field({
  kind,
  light,
  className,
  style,
}: {
  kind: FieldKind;
  light: boolean;
  className?: string;
  style?: CSSProperties;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const redrawRef = useRef<(() => void) | null>(null);
  const paletteRef = useRef<Palette>(light ? PALETTES[kind].light : PALETTES[kind].dark);

  useEffect(() => {
    const canvas = canvasRef.current;
    const box = canvas?.parentElement;
    const gl = canvas?.getContext("webgl", {
      alpha: true,
      antialias: false,
      premultipliedAlpha: true,
    });
    if (!canvas || !box || !gl) return;
    const linked = program(gl);
    if (!linked) return;
    const uniform = (name: string) => gl.getUniformLocation(linked, name);
    gl.uniform1f(uniform("cell"), CELL);
    gl.uniform1f(uniform("mode"), MODES[kind]);
    const start = performance.now() - Math.random() * START_JITTER_MS;
    const fit = () => {
      const width = Math.ceil(box.clientWidth / CELL);
      const height = Math.ceil(box.clientHeight / CELL);
      if (canvas.width === width && canvas.height === height) return;
      canvas.width = width;
      canvas.height = height;
      canvas.style.width = `${width * CELL}px`;
      canvas.style.height = `${height * CELL}px`;
      gl.viewport(0, 0, width, height);
      gl.uniform2f(uniform("res"), width, height);
    };
    const draw = (now: number) => {
      fit();
      paletteRef.current.forEach((color, index) =>
        gl.uniform3fv(uniform(PALETTE_UNIFORMS[index]), rgb(color)),
      );
      gl.uniform1f(uniform("time"), (now - start) / MS_PER_SECOND);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.drawArrays(gl.TRIANGLES, 0, TRIANGLE_VERTICES);
      canvas.dataset.on = "";
    };
    if (window.matchMedia(REDUCED_MOTION).matches) {
      const still = () => draw(start + STILL_TIME_MS);
      redrawRef.current = still;
      const observer = new ResizeObserver(still);
      observer.observe(box);
      return () => {
        redrawRef.current = null;
        observer.disconnect();
      };
    }
    let frame = 0;
    let last = 0;
    const tick = (now: number) => {
      if (now - last > FRAME_INTERVAL_MS) {
        last = now;
        draw(now);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [kind]);

  useEffect(() => {
    paletteRef.current = light ? PALETTES[kind].light : PALETTES[kind].dark;
    redrawRef.current?.();
  }, [kind, light]);

  return (
    <div
      aria-hidden="true"
      className={cn("pointer-events-none absolute inset-x-0 overflow-hidden", className)}
      style={style}
    >
      <canvas
        ref={canvasRef}
        className="pixelated absolute bottom-0 left-0 block opacity-0 transition-opacity duration-900 ease-out data-on:opacity-100"
      />
    </div>
  );
}
