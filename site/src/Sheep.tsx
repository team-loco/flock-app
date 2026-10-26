import type { CSSProperties } from "react";
import { cn } from "@/lib/utils";

const FRAMES = {
  base: "/sheep/base.svg",
  blink: "/sheep/blink.svg",
  stomp: "/sheep/stomp.svg",
};

const SPRITE = { width: 28, height: 22 };
const FRAME_HALF_WIDTH = 556;

type Sheep = {
  scale: number;
  offset: number;
  flip: boolean;
  walk: boolean;
  blinkDelay: string;
};

const FLOCK: Sheep[] = [
  { scale: 3, offset: -FRAME_HALF_WIDTH - 96, flip: false, walk: false, blinkDelay: "-1.2s" },
  { scale: 2, offset: -FRAME_HALF_WIDTH - 150, flip: true, walk: false, blinkDelay: "-3.1s" },
  { scale: 3, offset: 568, flip: true, walk: false, blinkDelay: "-0.4s" },
  { scale: 2, offset: 660, flip: false, walk: true, blinkDelay: "-2.2s" },
];

const frame = "pixelated absolute inset-0 block size-full";

function Frame({
  src,
  width,
  height,
  className,
  style,
}: {
  src: string;
  width: number;
  height: number;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <img
      src={src}
      alt=""
      width={width}
      height={height}
      className={cn(frame, className)}
      style={style}
    />
  );
}

export function Pasture({ band, className }: { band: string; className?: string }) {
  return (
    <div
      aria-hidden="true"
      className={cn("pointer-events-none absolute inset-0 overflow-hidden", className)}
    >
      {FLOCK.map((sheep) => {
        const width = SPRITE.width * sheep.scale;
        const height = SPRITE.height * sheep.scale;
        const place: CSSProperties = {
          width,
          height,
          top: `calc(${band} - ${height}px)`,
          left: `calc(50% + ${sheep.offset}px)`,
        };
        return (
          <div key={sheep.offset} className="absolute" style={place}>
            <div
              className={cn(
                "absolute inset-0",
                sheep.flip && "-scale-x-100",
                sheep.walk && "motion-safe:animate-walk",
              )}
            >
              <Frame src={FRAMES.base} width={width} height={height} />
              {sheep.walk && (
                <Frame
                  src={FRAMES.stomp}
                  width={width}
                  height={height}
                  className="opacity-0 motion-safe:animate-stomp"
                />
              )}
              <Frame
                src={FRAMES.blink}
                width={width}
                height={height}
                className="opacity-0 motion-safe:animate-blink"
                style={{ animationDelay: sheep.blinkDelay }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}
