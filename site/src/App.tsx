import { useState, useSyncExternalStore, type CSSProperties } from "react";
import { Button } from "@/components/ui/button";
import {
  CONNECT,
  CONNECT_TITLE,
  DOWNLOAD_NOTE,
  FEATURE_DISPLAY_WIDTH,
  HERO_ALT,
  HERO_DISPLAY_WIDTH,
  HERO_SCREEN,
  ICON_ASSET_SIZE,
  iconPath,
  KICKER,
  PIXEL_RATIO,
  shotHeight,
  shotPath,
  START,
  START_TITLE,
  STEPS,
  STORY,
  TAGLINE,
  TOOLS,
  TOOLS_TITLE,
  type Accent,
  type Screen,
  type Step,
  type Tool,
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
const STEP_DIGITS = 2;

const ACCENT_CLASS: Record<Accent, string> = {
  primary: "text-primary",
  violet: "text-violet",
  ok: "text-ok",
  err: "text-err",
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

function GitHubIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
    </svg>
  );
}

function Downloads({ inverse }: { inverse?: boolean }) {
  return (
    <div className="flex flex-wrap items-center justify-center gap-3">
      <Button asChild variant={inverse ? "inverse" : "default"}>
        <a href={__DOWNLOAD_URL__}>
          <DownloadIcon />
          Download for macOS
        </a>
      </Button>
      <Button asChild variant={inverse ? "inverse-outline" : "outline"}>
        <a href={__REPO_URL__}>
          <GitHubIcon />
          View on GitHub
        </a>
      </Button>
    </div>
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
  screen: Screen;
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
    return <img src={shotPath(screen, appearance)} alt={alt} loading={loading} className={image} {...size} />;
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
        <div className="relative overflow-hidden rounded-frame border border-frame-border bg-card shadow-frame">
          <div className="group relative">
            <Shot screen={HERO_SCREEN} width={HERO_DISPLAY_WIDTH} appearance="dark" alt={HERO_ALT} eager />
            <div className="absolute inset-0" style={{ clipPath: `inset(0 0 0 ${at})` }}>
              <Shot screen={HERO_SCREEN} width={HERO_DISPLAY_WIDTH} appearance="light" alt={HERO_ALT} eager />
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

function StepRow({ step, index }: { step: Step; index: number }) {
  return (
    <div className="border-b border-border">
      <div
        className={cn(
          "mx-auto flex max-w-page flex-col gap-10 px-6 py-18 lg:flex-row lg:items-center lg:gap-14",
          index % 2 === 1 && "lg:flex-row-reverse",
        )}
      >
        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <span className={cn("text-kicker font-semibold uppercase", ACCENT_CLASS[step.accent])}>
            <span className="font-mono">{String(index + 1).padStart(STEP_DIGITS, "0")}</span>
            {" · "}
            {step.kicker}
          </span>
          <h3 className="m-0 text-feature font-semibold">{step.title}</h3>
          <p className="m-0 text-body text-foreground-soft">{step.body}</p>
          <ul className="mt-1.5 flex list-none flex-col gap-1.5 p-0 font-mono text-point text-foreground-soft">
            {step.points.map((point) => (
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
          <Shot screen={step.screen} width={FEATURE_DISPLAY_WIDTH} alt={step.alt} />
        </div>
      </div>
    </div>
  );
}

function Cards({ title, cards, columns }: { title: string; cards: Tool[]; columns: string }) {
  return (
    <div className="flex flex-col gap-6">
      <h2 className="m-0 text-section font-semibold">{title}</h2>
      <ul
        className={cn(
          "m-0 grid list-none gap-px overflow-hidden rounded-shot border border-border bg-border p-0",
          columns,
        )}
      >
        {cards.map((card) => (
          <li key={card.title} className="flex flex-col gap-1.5 bg-background p-5">
            <h3 className="m-0 text-body font-semibold">{card.title}</h3>
            <p className="m-0 text-point text-foreground-soft">{card.body}</p>
          </li>
        ))}
      </ul>
    </div>
  );
}

export default function App() {
  const light = useSyncExternalStore(subscribeScheme, isLight);
  const year = new Date().getFullYear();
  return (
    <div className="flex min-h-screen w-full flex-col">
      <header className="relative lg:bg-hero-split" style={{ "--band-stop": BAND_STOP } as CSSProperties}>
        <Field kind="hero" light={light} className="top-0 max-lg:hidden" style={{ height: BAND_STOP }} />
        <section className="relative mx-auto flex max-w-page flex-col items-center gap-5.5 px-6 pt-11 pb-11 text-center">
          <Icon size={ICON_SIZE} label="Flock" className="shadow-icon" />
          <span className="text-kicker font-semibold text-primary uppercase">{KICKER}</span>
          <h1 className="m-0 max-w-headline text-hero-compact font-semibold sm:text-hero">
            Find what's broken. See what changed. <span className="text-primary">Across every cluster.</span>
          </h1>
          <p className="m-0 max-w-lead text-lead text-foreground-soft">{TAGLINE}</p>
          <div id="download" className="mt-1.5">
            <Downloads />
          </div>
          <span className="text-note text-muted-foreground">{DOWNLOAD_NOTE}</span>
        </section>
        <Compare />
        <Pasture band={BAND_STOP} className="max-lg:hidden" />
      </header>

      <main id="features">
        <section aria-labelledby="story" className="border-b border-border">
          <div className="mx-auto flex max-w-page flex-col gap-3 px-6 pt-18 pb-4 text-center">
            <h2 id="story" className="m-0 text-section font-semibold">
              {STORY.title}
            </h2>
            <p className="m-0 mx-auto max-w-lead text-body text-foreground-soft">{STORY.body}</p>
          </div>
        </section>
        {STEPS.map((step, index) => (
          <StepRow key={step.kicker} step={step} index={index} />
        ))}
        <section className="mx-auto flex max-w-page flex-col gap-18 px-6 py-18">
          <Cards title={TOOLS_TITLE} cards={TOOLS} columns="sm:grid-cols-2 lg:grid-cols-4" />
          <Cards title={CONNECT_TITLE} cards={CONNECT} columns="sm:grid-cols-2" />
          <div className="flex flex-col gap-6">
            <h2 className="m-0 text-section font-semibold">{START_TITLE}</h2>
            <ol className="m-0 grid list-none gap-4 p-0 sm:grid-cols-3">
              {START.map((line, index) => (
                <li key={line} className="flex gap-3 text-body text-foreground-soft">
                  <span className="font-mono text-primary">{index + 1}</span>
                  {line}
                </li>
              ))}
            </ol>
          </div>
        </section>
      </main>

      <section className="relative flex flex-[1_0_auto] flex-col bg-band text-band-foreground">
        <Field kind="cta" light={light} className="inset-y-0" />
        <div className="relative mx-auto flex max-w-page flex-col items-center gap-4 px-6 pt-12 pb-8 text-center">
          <Downloads inverse />
          <span className="text-note text-band-foreground-soft">{DOWNLOAD_NOTE}</span>
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
