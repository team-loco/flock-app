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

export type Accent = "primary" | "violet" | "ok" | "err" | "warn";

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
    title: "Every cluster in one table",
    body: "Flock is multi-cluster from the ground up. Every cluster stays connected, so their state, capacity and problems sit next to each other instead of behind a context switch.",
    points: [
      "all clusters connected at once",
      "compare clusters side by side in split panes",
      "native, written in Rust",
    ],
    screen: "flock",
    alt: "Flock listing every connected cluster with its status, capacity and problems",
  },
  {
    kicker: "Metrics",
    accent: "violet",
    title: "Resources and metrics, visualized",
    body: "CPU, memory and pod capacity for every node, pool and namespace, with requests and limits drawn on the same bar.",
    points: [
      "live usage per node and namespace",
      "requests vs. allocatable per pool",
      "history over 1h, 6h, 24h, 7d",
    ],
    screen: "nodes",
    alt: "Node list with CPU, memory and pod usage bars",
  },
  {
    kicker: "Logs and terminal",
    accent: "ok",
    title: "Real-time logs and a terminal for every pod",
    body: "Logs stream as they are written, parsed into fields. Open a shell in any container from the same window.",
    points: [
      "real-time log streaming",
      "logs from before the last restart",
      "terminal in any container",
    ],
    screen: "logs",
    alt: "Streaming pod logs parsed into fields",
  },
  {
    kicker: "Problems",
    accent: "err",
    title: "Errors as Kubernetes reports them",
    body: "Each failing object with its reason, message and age, straight from the API. Filter by severity, open the object in one click.",
    points: ["grouped by cluster", "conditions and events alongside", "one list for every cluster"],
    screen: "fleet-problems",
    alt: "Problems from every cluster, grouped by cluster, with an inspector showing events",
  },
  {
    kicker: "Helm",
    accent: "warn",
    title: "Helm releases and Flux",
    body: "Every Helm release with its chart, revision history and values. Flux Kustomizations and HelmReleases for every cluster, with the commit that changed them.",
    points: [
      "chart and app version per release",
      "release history and values",
      "reconcile, suspend, diff for Flux",
    ],
    screen: "helm-releases",
    alt: "Helm releases across clusters with chart and app versions",
  },
  {
    kicker: "Port forwarding and editing",
    accent: "primary",
    title: "Port forwards and a resource editor",
    body: "Forward a port from any pod or service. Edit any resource as YAML.",
    points: [
      "port-forward from ⌘K",
      "YAML editor for any resource",
      "VS Code shortcuts: ⌘P, ⌘⇧P, ⌘B, ⌘J",
    ],
    screen: "command",
    alt: "Command palette open over a cluster",
  },
];

export const shotPath = (screen: string, appearance: Appearance) =>
  `/shots/${screen}-${appearance}.webp`;

export const shotHeight = (width: number) =>
  Math.round((width * SHOT_SOURCE.height) / SHOT_SOURCE.width);
