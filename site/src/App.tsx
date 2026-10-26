import { useState, useSyncExternalStore, type CSSProperties } from "react";
import { Button } from "@/components/ui/button";
import {
  FEATURE_DISPLAY_WIDTH,
  FEATURES,
  ICON_ASSET_SIZE,
  iconPath,
  HERO_DISPLAY_WIDTH,
  HERO_SCREEN,
  PIXEL_RATIO,
  shotHeight,
  shotPath,
  TAGLINE,
  type Accent,
  type Feature,
} from "@/content";
import { Field } from "@/Field";
import { Pasture } from "@/Sheep";
import { cn } from "@/lib/utils";

const LIGHT_QUERY = "(prefers-color-scheme: light)";
const SPLIT_MIN = 5;
const SPLIT_MAX = 95;
const SPLIT_START = 50;

const BAND_STOP = "62%";
const COMPARE_BAND_STOP = "50%";
const ICON_SIZE = 84;
const CTA_ICON_SIZE = 88;

const ACCENT_CLASS: Record<Accent, string> = {
  primary: "text-primary",
  violet: "text-violet",
  ok: "text-ok",
  err: "text-err",
  warn: "text-warn",
};

const subscribeScheme = (onChange: () => void) => {
  const query = window.matchMedia(LIGHT_QUERY);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
};

const isLight = () => window.matchMedia(LIGHT_QUERY).matches;

function DownloadIcon() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 2v8.5M4.5 7 8 10.5 11.5 7M3 13.5h10" />
    </svg>
  );
}

function Icon({ size, label, className }: { size: number; label: string; className?: string }) {
  return (
    <span
      role="img"
      aria-label={label}
      className={cn("relative block flex-none overflow-hidden rounded-icon", className)}
      style={{ width: size, height: size }}
    >
      <picture>
        <source srcSet={iconPath(ICON_ASSET_SIZE, "webp")} type="image/webp" />
        <img
          src={iconPath(ICON_ASSET_SIZE, "png")}
          alt=""
          width={ICON_ASSET_SIZE}
          height={ICON_ASSET_SIZE}
          className="icon-crop block"
        />
      </picture>
    </span>
  );
}

function Shot({
  screen,
  width,
  alt,
  appearance,
  eager,
}: {
  screen: string;
  width: number;
  alt: string;
  appearance?: "dark" | "light";
  eager?: boolean;
}) {
  const pixels = width * PIXEL_RATIO;
  const loading = eager ? "eager" : "lazy";
  const size = { width: pixels, height: shotHeight(pixels) };
  const image = "block h-auto w-full";
  if (appearance) {
    return (
      <img
        src={shotPath(screen, appearance)}
        alt={alt}
        loading={loading}
        className={image}
        {...size}
      />
    );
  }
  return (
    <picture>
      <source media={LIGHT_QUERY} srcSet={shotPath(screen, "light")} />
      <img src={shotPath(screen, "dark")} alt={alt} loading={loading} className={image} {...size} />
    </picture>
  );
}

function Compare() {
  const [split, setSplit] = useState(SPLIT_START);
  const at = `${split}%`;
  return (
    <section
      aria-label="Screenshot, dark and light"
      className="relative px-6 pt-4 pb-12 max-lg:bg-hero-split lg:pb-26"
      style={{ "--band-stop": COMPARE_BAND_STOP } as CSSProperties}
    >
      <div className="mx-auto max-w-page">
        <div className="relative overflow-hidden rounded-frame border border-border-strong bg-card shadow-frame">
          <div className="group relative">
            <Shot
              screen={HERO_SCREEN}
              width={HERO_DISPLAY_WIDTH}
              appearance="dark"
              alt="Flock cluster overview in dark appearance"
              eager
            />
            <div className="absolute inset-0" style={{ clipPath: `inset(0 0 0 ${at})` }}>
              <Shot
                screen={HERO_SCREEN}
                width={HERO_DISPLAY_WIDTH}
                appearance="light"
                alt="Flock cluster overview in light appearance"
                eager
              />
            </div>
            <div
              aria-hidden="true"
              className="absolute inset-y-0 w-divider -translate-x-1/2 bg-handle shadow-handle"
              style={{ left: at }}
            />
            <div
              aria-hidden="true"
              className="absolute top-1/2 flex size-handle -translate-1/2 items-center justify-center rounded-full border-2 border-handle bg-background text-foreground group-has-focus-visible:ring-2 group-has-focus-visible:ring-ring"
              style={{ left: at }}
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <path d="M6 4 2 8l4 4M10 4l4 4-4 4" />
              </svg>
            </div>
            <input
              type="range"
              min={SPLIT_MIN}
              max={SPLIT_MAX}
              value={split}
              onChange={(event) => setSplit(Number(event.target.value))}
              aria-label="Compare dark and light"
              className="absolute inset-0 m-0 size-full cursor-ew-resize opacity-0"
            />
          </div>
        </div>
        <div className="mt-3.5 flex items-center justify-between font-mono text-caption text-band-foreground-soft">
          <span>Dark</span>
          <span>Light</span>
        </div>
      </div>
    </section>
  );
}

function FeatureRow({ feature, flip }: { feature: Feature; flip: boolean }) {
  return (
    <div className="border-b border-border">
      <div
        className={cn(
          "mx-auto flex max-w-page flex-col gap-10 px-6 py-18 lg:flex-row lg:items-center lg:gap-14",
          flip && "lg:flex-row-reverse",
        )}
      >
        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <span className={cn("text-kicker font-semibold uppercase", ACCENT_CLASS[feature.accent])}>
            {feature.kicker}
          </span>
          <h2 className="m-0 text-feature font-semibold">{feature.title}</h2>
          <p className="m-0 text-body text-foreground-soft">{feature.body}</p>
          <ul className="mt-1.5 flex list-none flex-col gap-1.5 p-0 font-mono text-point text-foreground-soft">
            {feature.points.map((point) => (
              <li key={point} className="flex gap-2.5">
                <span aria-hidden="true" className="text-muted-foreground">
                  —
                </span>
                {point}
              </li>
            ))}
          </ul>
        </div>
        <div className="w-full min-w-0 overflow-hidden rounded-shot border border-border-strong bg-card shadow-shot lg:w-shot lg:flex-none">
          <Shot screen={feature.screen} width={FEATURE_DISPLAY_WIDTH} alt={feature.alt} />
        </div>
      </div>
    </div>
  );
}

export default function App() {
  const light = useSyncExternalStore(subscribeScheme, isLight);
  const year = new Date().getFullYear();
  return (
    <div className="flex min-h-screen w-full flex-col">
      <header
        className="relative lg:bg-hero-split"
        style={{ "--band-stop": BAND_STOP } as CSSProperties}
      >
        <Field
          kind="hero"
          light={light}
          className="top-0 max-lg:hidden"
          style={{ height: BAND_STOP }}
        />
        <section className="relative mx-auto flex max-w-page flex-col items-center gap-5.5 px-6 pt-11 pb-11 text-center">
          <Icon size={ICON_SIZE} label="Flock" className="shadow-icon" />
          <h1 className="m-0 max-w-headline text-hero-compact font-semibold sm:text-hero">
            Kubernetes, without the{" "}
            <span className="text-muted-foreground line-through decoration-primary decoration-(length:--spacing-strike)">
              context switching
            </span>
            .
          </h1>
          <p className="m-0 max-w-lead text-lead text-foreground-soft">{TAGLINE}</p>
          <div id="download" className="mt-1.5 flex flex-wrap items-center justify-center gap-3">
            <Button asChild>
              <a href={__DOWNLOAD_URL__}>
                <DownloadIcon />
                Download for macOS
              </a>
            </Button>
          </div>
          <span className="text-note text-muted-foreground">For Macs with Apple silicon</span>
        </section>
        <Compare />
        <Pasture band={BAND_STOP} className="max-lg:hidden" />
      </header>

      <main id="features">
        {FEATURES.map((feature, index) => (
          <FeatureRow key={feature.screen} feature={feature} flip={index % 2 === 1} />
        ))}
      </main>

      <section className="relative flex flex-[1_0_auto] flex-col bg-band text-band-foreground">
        <Field kind="cta" light={light} className="inset-y-0" />
        <div className="relative mx-auto flex max-w-page flex-col items-center gap-4 px-6 pt-12 pb-8 text-center">
          <Icon size={CTA_ICON_SIZE} label="Flock app icon" />
          <h2 className="m-0 text-download font-semibold">Download Flock</h2>
          <div className="flex flex-wrap items-center justify-center gap-3">
            <Button asChild variant="inverse">
              <a href={__DOWNLOAD_URL__}>
                <DownloadIcon />
                Download for macOS
              </a>
            </Button>
          </div>
          <span className="text-note text-band-foreground-soft">For Macs with Apple silicon</span>
        </div>
        <footer className="relative mt-auto">
          <div className="mx-auto flex max-w-page flex-wrap items-center gap-6 px-6 pb-2 text-footer text-band-foreground-soft">
            <span className="font-medium text-band-foreground">Flock</span>
            <span className="flex-1" />
            <a href={__RELEASES_URL__} className="no-underline hover:text-band-foreground">
              Changelog
            </a>
            <a href={__REPO_URL__} className="no-underline hover:text-band-foreground">
              GitHub
            </a>
            <span>© {year}</span>
          </div>
        </footer>
      </section>
    </div>
  );
}
