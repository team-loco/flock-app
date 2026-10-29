# Flock design handoff (GPUI)

Native desktop app for debugging Kubernetes clusters, with first-class Loco support behind a build flag.
Each screen is in `screens/`: the markup holds the layout and inline styles, and the `<script>` holds the sample data and interactions. These files need the design canvas's runtime, so they won't render on their own in a browser. Read them as a spec and rebuild in GPUI; don't port the HTML.

## Layout (every window)

```
┌──────────────────┬──────────────────────────────────────────────────────────┐
│ ● ● ●            │ ✕ deploy checkout-api us-east × │ ks apps-payments … │ ⫿ │ pane tabs 32px
├──────────────────┤──────────────────────────────────────────────────────────┤
│ prod-us-east   « │ page header (44px): name · status                        │
│ Overview         │   one primary action + ⋯ menu                            │
│ Nodes     ▲1 14  │                                                          │
│ Namespaces   23  │ content (scrolls)                                        │
│ Events       ▲7  │                                                          │
│ LOCO (installed) │                                                          │
│ WORKLOADS / FLUX │                                                          │
│ 200px            │                                                          │
├──────────────────┴──────────────────────────────────────────────────────────┤
│ ✕3 ▲6 · EKS · us-east-1 · v1.31.2      ● loco agent · synced 4s ago │ ✕ prod-us-east ^ │
└─────────────────────────────────────────────────────────────────────────────┘
```

- One sidebar, 200px, under the 28px strip (window controls). Its header row names the scope (All clusters on Flock home, or the cluster name) with the collapse chevron. ⌘B collapses it to 76px: the strip, then an expand chevron over the scope name drawn as a vertical label.
- On Flock home the sidebar holds Overview, Problems, GitOps, Changes and Flux events, then a RESOURCES section (v2.1 `Nav mode=fleet exp`, decision 60) with Deployments, Pods, Nodes and Helm releases, each with ✕ / ▲ badges summed over the selected clusters; in a cluster it holds that cluster's pages. The LOCO section appears only on clusters running Loco.
- The status bar (26px, `chrome`, 1px top `line`; v2.1 `StatusBarMulti`) starts with the problem counts of the selected clusters (✕ 3 ▲ 6, hover background, opens Problems, 1px rule on the right), then the Kubernetes version only when exactly one cluster is selected, a spacer, the 32px icon-only Refresh, then the cluster selector at the right end after a 1px rule: a 2x2 squares icon (the fourth square faded when one cluster is selected), All clusters or the first selected cluster's name followed by a mono `+N`, and an up chevron; `sel` background while open. A click or ⌘⇧C opens a 300px menu above it, anchored to the right edge with a 6px inset (`StatusBarMultiOpen`): an All clusters checkbox (checked when all are selected, a dash when some are, with `n of m`), a rule, one checkbox row per cluster (name, then its Kubernetes version, or "auth expired" / "offline 3h" with the name in mute; a gear on hover opens the cluster menu), a rule, then Add cluster…. Clicking a row toggles it and the menu stays open; the last selected cluster cannot be deselected; All clusters selects all, or only the first when all are selected. Space toggles the highlighted row, Enter picks only it and closes, ⌘0 selects all clusters. Switching never disconnects a cluster.
- Plain cluster names with a status glyph. No icons or avatars.
- Every content area has a pane tab strip (`PaneTabs`, 32px, `chrome` background). A tab is one open view: a Flock page (Overview, GitOps, Changes), a cluster page or object (deploy, ks, loco, …) or a pod's logs. Each tab shows the object's status glyph when it has one, a mono kind label (`flock`, `deploy`, `ks`, `logs`, `loco`), the name, and the short cluster name in mute. The active tab takes the `editor` background and joins the content; in the focused pane it also gets a 1px `acc` line on top. The close × shows on the active tab. The strip ends with a Split right button (⌘\).
- Split right divides the content area into side-by-side panes of equal width, separated by a 1px `line2` rule; each pane has its own tab strip and can show a different cluster. Clicking a tab focuses its pane. The sidebar and the status-bar cluster selector follow the focused pane; each pane's tab keeps its own selection. ⌘W closes the active tab.
- No breadcrumbs, no panel-layout buttons. ⌘K opens the command palette from the keyboard; there is no visible button for it.
- One visible primary action per page (for example Reconcile). Everything else goes in a ⋯ / Actions menu, each item with a one-line description.
- Inspectors slide and fade in from the right and out again when closed.

## Visual rules

- Font: IBM Plex Sans for UI at 13px, IBM Plex Mono for data at 12px. Uppercase 11px section labels.
- Corners: 3px radius maximum, 4px for menus and dialogs. No pills, no cards. Sections are separated by 1px rules.
- Status uses both shape and colour: ● ok, ▲ warning, ✕ error, ◐ in progress, ‖ suspended, ○ offline. ○ in mute also marks a Loco build that ended without finishing (canceled or superseded); it is neither an error nor a success.
- Pages read like documents. Put one plain sentence about what's wrong first, then data. Diffs are always shown in full. (List pages drop the sentence and open on the filter bar: decision 22 in `docs/decisions.md`.)
- Light and dark follow the OS. Every colour is a token; the values are below.

| token | dark | light | use |
|---|---|---|---|
| editor | #111317 | #FFFFFF | content background |
| panel | #15181D | #F6F7F8 | side panels, inspectors |
| chrome | #1A1D22 | #EEEFF1 | status bar, strip, popover footers |
| hover | #1D2127 | #F1F2F4 | row hover |
| line / line2 | #262A30 / #323740 | #E1E3E7 / #CFD3D9 | rules / control borders |
| fg / fg2 / mute | #DCDFE4 / #B3B9C3 / #8E96A3 | #1D2027 / #3B414B / #5F6774 | text levels |
| sel / selfg / selline | #1F2C40 / #F2F6FC / #3A5A8C | #DCE7FA / #0D2850 / #8DB0EA | selection |
| acc / onacc | #74ADFF / #0C1220 | #1F5FD1 / #FFFFFF | links, primary button |
| ok / warn / err / sus | #48C78E / #E8B04B / #F26D6D / #9AA2AE | #167A4C / #935800 / #C53030 / #6B7280 | status |
| okbg / warnbg / errbg | ok/warn/err at 10–13% | 7–10% | tinted rows, diff +/- |

Row heights: 24–28px for tables, 26px for nav rows. Use virtualized lists (`uniform_list`) for every table.

## Screens (`screens/`)

**Loco (shown only when built with the loco flag)**
- `LocoPlatform`: agent ↔ control-plane Sync stream, last heartbeat, the health it reports (kubernetes, controller), reconnects, builds enabled. Placements out of sync (apply error/retrying, env secret pending). Component table: agent, loco-controller, loco-build-controller, envoy-gateway and proxy, cert-manager, cilium, otel, clickhouse, observability-proxy. Agent log.
- `LocoApps`: Application CRs (infra.loco.io/v1alpha1) grouped by workspace (`ws-<id>` namespace). The detail panel shows phase, the controller's own Degraded message, image and deployment, desired vs applied placement revision, and the objects the controller created (Deployment, Service, HTTPRoute, env Secret, SA, NetworkPolicies), plus the route path.
- `LocoBuilds`: Build CRs run as Jobs in `loco-builds` (fetch init container + buildkit), with phase, duration, digest and cache hit, and a log pane.
- `LocoRoutes`: listeners on Gateway `eg`, then HTTPRoutes with problems first (no ready endpoints, BackendNotFound).
- Open question: the cluster only knows `resource-<uuid>`. The design assumes friendly names come from the user's `loco login` session against the Loco API.

**Triage flow**: `Flock` → `Command` (⌘K) → `Workload` → `Kustomization` → `Changes`
- `Flock`: clusters table, problems grouped by cluster, recent GitOps commits with per-cluster outcome.
- `Workload`: deployment page with pods, rollout history linked to commits, resolved env vars (flags a missing secret key), and a logs/events/terminal dock.
- `Kustomization`: Flux object page with one-sentence status, managed objects, history, the same Kustomization on other clusters, and Spec at the bottom.
- `Changes`: commits with per-cluster outcome, plus a full diff with source and per-cluster rendered views.

**Other**: `Flux` (sync grid across clusters), `AddCluster` and `AddClusterCheck` (pick kubeconfig contexts, then connection checks), `AppIcon`.
Light variants: `*Light.dc.html`.
**Tabs, split and switcher**: `Split` (two panes, the same deployment on prod-us-east and prod-eu-west, with logs and Kustomization tabs), `ClusterSwitcher` (the Workload page with the switcher menu open).
Shared parts: `Nav` (the sidebar), `StatusBar` (with the cluster switcher), `PaneTabs` (the tab strip).

`assets/app-icon.png` is the app icon: a finished macOS tile (rounded square, drop shadow) on an opaque white square. Don't add another squircle; the white surround must become transparent for the .icns.
