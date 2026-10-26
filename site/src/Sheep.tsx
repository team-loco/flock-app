import { useState, type CSSProperties } from "react";
import { cn } from "@/lib/utils";

const FRAMES = {
  base: "/sheep/base.svg",
  blink: "/sheep/blink.svg",
  stomp: "/sheep/stomp.svg",
};

const SPRITE = { width: 28, height: 22 };
const EDGE_MARGIN = 4;
const GUTTER = "(50% - min(50%, var(--container-page) / 2) + var(--spacing) * 6)";

type Side = "left" | "right";

type SheepSpec = {
  side: Side;
  scale: number;
  reach: number;
  flip: boolean;
  walk: boolean;
  blinkDelay: string;
};

const FLOCK: SheepSpec[] = [
  { side: "left", scale: 3, reach: 96, flip: false, walk: false, blinkDelay: "-1.2s" },
  { side: "left", scale: 2, reach: 150, flip: true, walk: false, blinkDelay: "-3.1s" },
  { side: "right", scale: 3, reach: 96, flip: true, walk: false, blinkDelay: "-0.4s" },
  { side: "right", scale: 2, reach: 160, flip: false, walk: true, blinkDelay: "-2.2s" },
];

const sideReach = (side: Side) =>
  Math.max(...FLOCK.filter((sheep) => sheep.side === side).map((sheep) => sheep.reach));

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

function Sheep({ sheep, band }: { sheep: SheepSpec; band: string }) {
  const [jumps, setJumps] = useState(0);
  const width = SPRITE.width * sheep.scale;
  const height = SPRITE.height * sheep.scale;
  const reach = sideReach(sheep.side);
  const place: CSSProperties = {
    width,
    height,
    top: `calc(${band} - ${height}px)`,
    [sheep.side]: `calc(max(${EDGE_MARGIN}px, ${GUTTER} - ${reach}px) + ${reach - sheep.reach}px)`,
  };
  const jumping = jumps > 0;
  return (
    <button
      type="button"
      aria-label="Make the sheep jump"
      data-sheep={sheep.side}
      onClick={() => setJumps((count) => count + 1)}
      className="pointer-events-auto absolute cursor-pointer border-0 bg-transparent p-0 outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      style={place}
    >
      <span className={cn("absolute inset-0 block", sheep.walk && "motion-safe:animate-walk")}>
        <span
          key={jumps}
          data-jump=""
          className={cn(
            "absolute inset-0 block",
            jumping && "motion-safe:animate-hop motion-reduce:animate-hop-small",
          )}
        >
          <span className={cn("absolute inset-0 block", sheep.flip && "-scale-x-100")}>
            <Frame src={FRAMES.base} width={width} height={height} />
            {sheep.walk && (
              <Frame
                src={FRAMES.stomp}
                width={width}
                height={height}
                className="opacity-0 motion-safe:animate-stomp"
              />
            )}
            {jumping && (
              <Frame
                src={FRAMES.stomp}
                width={width}
                height={height}
                className="opacity-0 animate-land"
              />
            )}
            <Frame
              src={FRAMES.blink}
              width={width}
              height={height}
              className="opacity-0 motion-safe:animate-blink"
              style={{ animationDelay: sheep.blinkDelay }}
            />
          </span>
        </span>
      </span>
    </button>
  );
}

export function Pasture({ band, className }: { band: string; className?: string }) {
  return (
    <div className={cn("pointer-events-none absolute inset-0 overflow-hidden", className)}>
      {FLOCK.map((sheep) => (
        <Sheep key={`${sheep.side}-${sheep.reach}`} sheep={sheep} band={band} />
      ))}
    </div>
  );
}
