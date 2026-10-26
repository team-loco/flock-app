export type Dot = {
  x: string;
  y: string;
  color: string;
  opacity: string;
  twinkle?: { opacity: string; duration: string; delay: string };
};

export type Tint = { color: string; opacity: number };

const MULBERRY_STEP = 0x6d2b79f5;
const UINT_RANGE = 4294967296;
const NOISE_GRID = 11;
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
const BAYER_SIZE = 4;
const BAYER_LEVELS = 16;
const CELL_CENTER = 0.5;
const THRESHOLD_JITTER = 0.1;
const DENSITY_GAIN = 2.4;
const NOISE_FLOOR = 0.3;
const NOISE_SPAN = 0.7;
const NOISE_EXPONENT = 1.6;
const DROP_MARGIN = 0.08;
const LIVE_MARGIN = 0.06;
const OPACITY_BASE = 0.7;
const OPACITY_SPREAD = 0.6;
const TWINKLE_MIN_SECONDS = 4;
const TWINKLE_SPREAD_SECONDS = 7;
const TWINKLE_DELAY_SECONDS = 11;
export const PERCENT = 100;
const POSITION_DIGITS = 3;
const OPACITY_DIGITS = 2;
const TIME_DIGITS = 1;

export const smoothstep = (t: number) => t * t * (3 - 2 * t);

export const smoothBetween = (a: number, b: number, x: number) =>
  smoothstep(Math.max(0, Math.min(1, (x - a) / (b - a))));

export function dots(
  seed: number,
  cols: number,
  rows: number,
  density: (u: number, v: number) => number,
  tint: (v: number) => Tint,
): Dot[] {
  let state = seed >>> 0;
  const random = () => {
    state = (state + MULBERRY_STEP) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / UINT_RANGE;
  };
  const grid: number[] = [];
  for (let i = 0; i < (NOISE_GRID + 1) * (NOISE_GRID + 1); i++) grid.push(random());
  const at = (i: number, j: number) => grid[j * (NOISE_GRID + 1) + i];
  const noise = (u: number, v: number) => {
    const x = u * NOISE_GRID;
    const y = v * NOISE_GRID;
    const i = Math.min(NOISE_GRID - 1, Math.floor(x));
    const j = Math.min(NOISE_GRID - 1, Math.floor(y));
    const fx = smoothstep(x - i);
    const fy = smoothstep(y - j);
    const top = at(i, j) * (1 - fx) + at(i + 1, j) * fx;
    const bottom = at(i, j + 1) * (1 - fx) + at(i + 1, j + 1) * fx;
    return top * (1 - fy) + bottom * fy;
  };
  const out: Dot[] = [];
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const u = (i + CELL_CENTER) / cols;
      const v = (j + CELL_CENTER) / rows;
      const n = noise(u, v);
      const shaped = Math.pow(Math.max(0, (n - NOISE_FLOOR) / NOISE_SPAN), NOISE_EXPONENT);
      const d = density(u, v) * DENSITY_GAIN * shaped;
      const bayer = BAYER[(j % BAYER_SIZE) * BAYER_SIZE + (i % BAYER_SIZE)];
      const threshold =
        (bayer + CELL_CENTER) / BAYER_LEVELS + (random() - CELL_CENTER) * THRESHOLD_JITTER;
      if (d < threshold - DROP_MARGIN) continue;
      const { color, opacity: base } = tint(v);
      const opacity = (base * (OPACITY_BASE + OPACITY_SPREAD * random())).toFixed(OPACITY_DIGITS);
      const live = d < threshold + LIVE_MARGIN;
      if (!live && d < threshold) continue;
      const x = (u * PERCENT).toFixed(POSITION_DIGITS);
      const y = (v * PERCENT).toFixed(POSITION_DIGITS);
      if (live) {
        const duration = (TWINKLE_MIN_SECONDS + random() * TWINKLE_SPREAD_SECONDS).toFixed(
          TIME_DIGITS,
        );
        const delay = (-random() * TWINKLE_DELAY_SECONDS).toFixed(TIME_DIGITS);
        out.push({
          x,
          y,
          color,
          opacity: "0",
          twinkle: { opacity, duration: `${duration}s`, delay: `${delay}s` },
        });
      } else {
        out.push({ x, y, color, opacity });
      }
    }
  }
  return out;
}
