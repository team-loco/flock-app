export type Appearance = "dark" | "light";

export const APPEARANCES: readonly Appearance[] = ["dark", "light"];

export const SHOT_SOURCE = { width: 1440, height: 900 } as const;

export const PIXEL_RATIO = 2;

export const HERO_DISPLAY_WIDTH = 1110;

export const FEATURE_DISPLAY_WIDTH = 618;

export type Screen = { name: string; group?: number };

export const HERO_SCREEN: Screen = { name: "flock-deployments", group: 3 };

export const HERO_ALT =
  "Deployments from two clusters in one table, grouped by namespace, with checkout-api failing on prod-us-east";

export const TITLE = "Flock: find what's broken and what changed across every Kubernetes cluster";

export const KICKER = "Native Kubernetes app for macOS";

export const TAGLINE =
  "Flock brings your Kubernetes workloads, failures, logs and Flux rollouts into one native desktop app. See what is happening across environments without jumping between clusters and tools.";

export const DOWNLOAD_NOTE = "Free and open source · Apache-2.0 · Apple Silicon Only";

export const ICON_ASSET_SIZE = 224;

export const iconPath = (size: number, format: "png" | "webp") => `/icon-${size}.${format}`;

export type Accent = "primary" | "violet" | "ok" | "err";

export type Step = {
  kicker: string;
  accent: Accent;
  title: string;
  body: string;
  points: string[];
  screen: Screen;
  alt: string;
};

export const STORY = {
  title: "One incident, start to finish",
  body: "checkout-api is failing in prod-us-east. This is the path from the first sign of trouble to the line of config that caused it, without leaving Flock.",
};

export const STEPS: Step[] = [
  {
    kicker: "Problems",
    accent: "err",
    title: "Find what's broken before you go looking",
    body: "Failing objects from every connected cluster land in one list, with the reason Kubernetes gives, the conditions behind it and the latest events beside it. Open a row to jump to the object in its own cluster.",
    points: [
      "Deployments, Pods, Nodes, Flux and Helm objects, grouped by cluster",
      "conditions and recent events next to each failure",
      "expired credentials and unreachable clusters show up too",
    ],
    screen: { name: "fleet-problems" },
    alt: "Problems from every cluster, grouped by cluster, with an inspector showing the failing condition and its events",
  },
  {
    kicker: "Across clusters",
    accent: "primary",
    title: "Compare it with the clusters where it works",
    body: "Put the same workload from every cluster side by side and hide what matches. Ready counts, images, env, replicas, resources and the last rollout line up, with a diff of the pod template underneath.",
    points: [
      "only the differences, from live values",
      "Deployments, Pods, Nodes and Helm releases from several clusters in one table",
      "restart, scale, cordon or delete many at once",
    ],
    screen: { name: "compare" },
    alt: "checkout-api compared across three clusters, showing only the fields that differ and a diff of the pod template",
  },
  {
    kicker: "Flux",
    accent: "violet",
    title: "Follow the change from Git to each cluster",
    body: "Flock reads your Flux Kustomizations, HelmReleases and sources, and lines every commit up with each cluster it reached: applied and healthy, failing its health check, or held back by a suspended Kustomization. The rendered diff shows what changed on that cluster.",
    points: [
      "rollout status for every commit on every cluster",
      "source and rendered diffs, with where each value is set",
      "dependency graph, drift, reconcile and suspend",
    ],
    screen: { name: "changes" },
    alt: "Commits with their rollout status on each cluster, and the rendered diff of the selected commit for prod-us-east",
  },
  {
    kicker: "Debugging",
    accent: "ok",
    title: "Debug the workload, not one pod at a time",
    body: "Stream logs from every pod of a workload, on every cluster, at once. Filter by source or level and spot error spikes over time. When you need to go further, open a shell, start a debug container or port-forward.",
    points: [
      "levels read from JSON and logfmt lines",
      "a debug container when the image has no shell",
      "port-forward and YAML editing from ⌘K",
    ],
    screen: { name: "logs" },
    alt: "Logs from every pod of a workload, with a volume histogram by level and the sources grouped by status",
  },
];

export type Tool = { title: string; body: string };

export const TOOLS_TITLE = "Everything else you reach for";

export const TOOLS: Tool[] = [
  {
    title: "Every cluster at a glance",
    body: "Each cluster's health, capacity and problem count on one page.",
  },
  {
    title: "Node allocation",
    body: "A treemap of every pod on a node, sized by CPU or memory requests or limits, with the room still unrequested.",
  },
  {
    title: "Menu-bar health",
    body: "Keep an eye on your clusters without opening Flock, with notifications when something breaks.",
  },
  {
    title: "Command palette",
    body: "Jump to any resource or run an action from the keyboard with ⌘K.",
  },
  {
    title: "Tabs and split panes",
    body: "Keep related resources side by side, each tab on its own clusters.",
  },
  {
    title: "YAML editing",
    body: "Inspect and edit any object's manifest in place.",
  },
  {
    title: "Helm history",
    body: "Every stored revision of a release, decoded from Helm storage, with its values.",
  },
  {
    title: "Native",
    body: "Written in Rust on GPUI and drawn with Metal. No web view, no Electron.",
  },
];

export const CONNECT_TITLE = "How Flock connects";

export const CONNECT: Tool[] = [
  {
    title: "Your kubeconfig",
    body: "Flock reads ~/.kube/config or $KUBECONFIG, follows changes to it, and runs exec plugins such as aws or gcloud with your login shell's environment.",
  },
  {
    title: "Nothing in the cluster",
    body: "No agent, operator or CRDs to install. Flock talks to the Kubernetes API directly.",
  },
  {
    title: "Your permissions",
    body: "Every read and every action uses your credentials, so RBAC decides what you can see and do.",
  },
  {
    title: "No account",
    body: "No sign-up and no telemetry. Outside your clusters, Flock only runs git with your credentials for Flux diffs and checks GitHub for updates.",
  },
];

export const START_TITLE = "Getting started";

export const START: string[] = [
  "Download the zip and move Flock to Applications.",
  "Open it. The clusters in your kubeconfig connect on their own.",
  "Pick one cluster or several from the status bar.",
];

export const shotName = ({ name, group }: Screen) => (group === undefined ? name : `${name}-group-${group}`);

export const shotPath = (screen: Screen, appearance: Appearance) => `/shots/${shotName(screen)}-${appearance}.webp`;

export const shotHeight = (width: number) => Math.round((width * SHOT_SOURCE.height) / SHOT_SOURCE.width);
