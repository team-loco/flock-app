export type Appearance = "dark" | "light";

export const APPEARANCES: readonly Appearance[] = ["dark", "light"];

export const SHOT_SOURCE = { width: 1440, height: 900 } as const;

export const PIXEL_RATIO = 2;

export const HERO_DISPLAY_WIDTH = 1110;

export const FEATURE_DISPLAY_WIDTH = 618;

export const HERO_SCREEN = "overview";

export const TITLE = "Flock: Kubernetes, without the context switching";

export const TAGLINE =
  "Flock is a native Mac app for running and debugging multi-cluster Kubernetes setups. All your clusters, side by side, in one window.";

export const ICON_ASSET_SIZE = 224;

export const iconPath = (size: number, format: "png" | "webp") => `/icon-${size}.${format}`;

export type Accent = "primary" | "violet" | "ok" | "err";

export type Feature = {
  kicker: string;
  accent: Accent;
  title: string;
  body: string;
  points: string[];
  screen: string;
  alt: string;
};

export const FEATURES: Feature[] = [
  {
    kicker: "Multi-cluster",
    accent: "primary",
    title: "Every cluster in one window",
    body: "Every cluster in your kubeconfig stays connected. Pick a workload and see it on all of them, with only the differences shown.",
    points: [
      "all clusters connected at once",
      "compare a workload across clusters",
      "native, written in Rust",
      "open source, Apache-2.0",
    ],
    screen: "flock",
    alt: "Flock listing every connected cluster with its status, capacity and problems",
  },
  {
    kicker: "GitOps",
    accent: "violet",
    title: "See which commit is on which cluster",
    body: "Flux Kustomizations, HelmReleases and sources for every cluster, linked to the commit that changed them.",
    points: [
      "commit-to-cluster tracing",
      "drift against what Flux last applied",
      "dependency graph; reconcile, suspend, diff",
    ],
    screen: "changes",
    alt: "Commits listed with their rollout status on each cluster, and the rendered diff of the selected commit",
  },
  {
    kicker: "Problems",
    accent: "err",
    title: "Everything that's broken, in one list",
    body: "Failing objects from every cluster, with the reason Kubernetes gives and the events next to it.",
    points: ["grouped by cluster", "conditions and events alongside", "cluster health in the menu bar"],
    screen: "fleet-problems",
    alt: "Problems from every cluster, grouped by cluster, with an inspector showing events",
  },
  {
    kicker: "Debugging",
    accent: "ok",
    title: "Logs, shells and debug containers",
    body: "Stream logs from every pod of a workload at once. Open a shell in any container, or a debug container when the image has none.",
    points: [
      "logs from all pods, parsed into fields",
      "logs from before the last restart",
      "port-forward and YAML editing from ⌘K",
    ],
    screen: "logs",
    alt: "Streaming pod logs parsed into fields",
  },
];

export const shotPath = (screen: string, appearance: Appearance) =>
  `/shots/${screen}-${appearance}.webp`;

export const shotHeight = (width: number) =>
  Math.round((width * SHOT_SOURCE.height) / SHOT_SOURCE.width);
