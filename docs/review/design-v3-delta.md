# Design v3 delta

Source: the Claude Design canvas (version 1791918775-00a4), synced into `design/` on 2026-10-13. Each entry compares the canvas board with the previous copy in `design/` (`git diff` of this sync). The canvas still says "Shoal"; decision 28 replaces it with Flock.

## NEW

| Board | Where | What it shows |
|---|---|---|
| `PaneTabs` | `design/screens` | Component: the 32px pane tab strip, with a Split right button at the end. |
| `Split` | `design/screens` | Tabs and two side-by-side panes. Left pane: checkout-api on us-east, with logs and Kustomization tabs. Right pane: checkout-api on eu-west, with a Kustomization tab. |
| `ClusterSwitcher` | `design/screens` | The Workload page with the status-bar switcher menu open (`Workload switcher=true`). |
| `CatalogLight`, `CompareLight`, `MainLight`, `PaletteLight`, `PodsLight` | `design/v1` | Light wrappers of v1 boards. The canvas already had them, but the earlier sync left them out. No design change. |

## CHANGED

| Board | Change |
|---|---|
| `canvas.json` | Adds Split, ClusterSwitcher (Loco row, y −1600) and PaneTabs (components row). Nav board is now 200px wide, titled "Component · sidebar" (was 360px, "clusters + resources"). The Loco title note now reads "… · tabs + split panes · cluster switcher". |
| `Nav` | The two-panel sidebar (164px clusters column and 196px resources column, ⌘⇧B) becomes one 200px sidebar. The header row shows the scope label (All clusters, or the cluster name) and a collapse chevron (⌘B). Collapsed, it is 76px: an expand chevron over a vertical scope label. The cluster list, All clusters row and Add cluster row are removed from it. `active` gains the `loco-*` keys. The nav items are unchanged. |
| `StatusBar` | The `right` text prop is removed. A cluster switcher button is added at the right end after a 1px `line` rule: status glyph, name or "All clusters", and an up chevron. Its tooltip is "Switch cluster ⌘⇧C" and it uses the `sel` background when open. The menu is 290px wide, anchored right 6px and 30px above the bar. It lists All clusters (⌘0), a rule, six clusters (glyph, name, meta such as `✕2 ▲3`, `‖1`, `▲2`, `auth expired`, `offline`, with offline names in mute), a rule, and Add cluster…. The current row uses `sel`. Hovering a row (or being the current row) shows a gear button labelled "Cluster settings". New props: `cluster` (flock or prod-us-east) and `open`. The `left` text now ellipsizes. |
| `Workload` | Gains a PaneTabs strip above the content: deploy checkout-api us-east ✕ (active), ks apps-payments us-east, deploy checkout-api eu-west. Nav hint 200px. StatusBar gets `cluster="prod-us-east"` and `open={{switcher}}`. New `switcher` prop, which the ClusterSwitcher board uses. |
| `Flock` | PaneTabs: flock Overview (active), flock GitOps, deploy checkout-api us-east ✕. Nav hint 200px. |
| `Flux` | PaneTabs: flock GitOps (active), flock Overview. Nav hint 200px. |
| `Changes` | PaneTabs: flock Changes (active), ks apps-payments us-east ✕. Nav hint 200px. |
| `Kustomization` | PaneTabs: deploy checkout-api ✕, ks apps-payments ✕ (active), logs x2lqp, all on us-east. The `clusters=false` prop is dropped from Nav (hint 224px → 200px). Main and Related columns become `calc(100% - 32px)`. StatusBar gets `cluster="prod-us-east"`. |
| `LocoPlatform` | PaneTabs: loco Platform (active), loco Applications. Nav hint 200px. StatusBar gets `cluster="prod-us-east"`. |
| `LocoApps` | PaneTabs: loco Applications (active), loco Platform, logs checkout. Columns become `calc(100% - 32px)`. Nav hint 200px. StatusBar gets `cluster`. |
| `LocoBuilds` | PaneTabs: loco Builds (active), loco Applications. Nav hint 200px. StatusBar gets `cluster`. |
| `LocoRoutes` | PaneTabs: loco Routes (active), loco Applications. Nav hint 200px. StatusBar gets `cluster`. |
| `AppIcon` | New artwork: a soft 3D sheep on a grassy hill under a blue sky. It is a finished rounded macOS tile, referenced as `/_blob/6d5ada19…`. The board no longer draws its own rounded tile (it had 90px/29px/14px radii and 118% zoom). The images use `object-fit: contain` at 460, 160, 80, 40 and 20px, on a white board labelled "Shoal.app · macOS icon", with sizes labelled 128 pt / 64 / 32 / 16. |
| `HANDOFF.md` | The layout section is rewritten for one sidebar, pane tabs, split and the right-hand switcher. The "No tabs" rule and the decision 18 superseded note are removed. The Screens list gains Split, ClusterSwitcher and PaneTabs. The icon note now describes the pre-masked tile. |
| `assets/flock-logo.png`, `design/assets/app-icon.png` | The new icon source (1254×1254 RGB PNG, sha256 `da8f8a6f…`) replaces the old logo. `design/assets/shoal-logo.png` is removed because nothing references it. |

## UNCHANGED

v2: `AddCluster`, `AddClusterCheck`, `Command` (all three import `Flock`, so they pick up its tab strip), `FlockLight`, `KustomizationLight`, `ChangesLight`.
v1: `Main`, `Pods`, `Catalog`, `Compare`, `Palette`, `Sidebar`, `Titlebar`.

## Tabs and panes: intended behaviour

- **What a tab is**: one open view of one target on one cluster. It is not tied to a sidebar entry. The kinds drawn are `flock` (Flock home pages: Overview, GitOps, Changes), `deploy` (a workload), `ks` (a Flux Kustomization), `logs` (one pod's log stream, labelled with the pod-hash suffix such as `x2lqp`) and `loco` (Loco pages: Platform, Applications, Builds, Routes). The same object on two clusters gives two tabs (deploy checkout-api us-east and eu-west). Flock pages carry no cluster; everything else carries the cluster's short name.
- **Tab anatomy** (`PaneTabs`): 32px strip on `chrome` with a bottom `line` rule. Each tab is a 12px-left / 8px-right padded button with a right `line` rule. It contains, in order: an optional status glyph (mono 10px, coloured: the object's health), the kind (mono 11px, mute), the label (12.5px) and the cluster (mute). It ends with a 14px ×, visible only on the active tab. Active tab: `editor` background, `fg` text, overlaps the bottom rule by 1px. In the focused pane it also gets an 1px `acc` bottom line. Inactive tabs are transparent with `mute` text. The strip overflows hidden (no scroll arrows are drawn). The strip's last item is a 32px "Split right ⌘\" button.
- **Panes**: Split right adds a pane to the right. Only a horizontal row of side-by-side panes is drawn, each `flex: 1 1 0` (equal widths), with a 1px `line2` left border on later panes. No vertical split, resize handle or pane count limit is drawn; the board shows 2. Each pane has its own tab list and active tab, and one pane is focused (`focus` state). Clicking a tab activates it and focuses that pane.
- **Content inside a pane**: a deploy tab in a split pane renders a compact deployment view: header with name, status and cluster, a one-sentence summary, the field list and Pods. It has no action bar, dock or inspector. A ks tab renders a summary with an "Open full view →" link. A logs tab renders the log lines. A single-pane window keeps the full page (Workload with dock, Kustomization with Related column, and so on).
- **Sidebar**: one sidebar per window, following the focused pane's cluster. In `Split` the sidebar is `prod-us-east` / Deployments while the left (focused) pane shows us-east.
- **Status-bar switcher**: shows the focused pane's cluster (`cluster="prod-us-east"` in Split). The board doesn't say whether switching the cluster retargets the active tab or opens a new one.
- **Shortcuts shown**: ⌘\ split right, ⌘W close tab, ⌘⇧C switcher, ⌘0 All clusters (in the switcher menu), ⌘B sidebar. The Split board's status-bar text is "2 panes · ⌘\ split · ⌘W close tab".
- **Persistence**: nothing in the design says tabs or panes persist.
- **Leftovers**: `Split` computes `comparing`, `hl`, `diffCount` and `toggleHl` (≠ marks on differing fields), but `hl` is hard-coded false and nothing renders them. Cross-pane diff highlighting was drafted and then turned off.

## Conflicts with decisions and HANDOFF

- **HANDOFF "No tabs"**: replaced by pane tabs on every page.
- **Decision 17 (split deferred)**: it requires extracting pane ownership (screens, dialogs, subscriptions, focus) from the shell before independent pane navigation. Split is now designed, so that refactor is a prerequisite.
- **Decision 18**:
  - (a) The switcher moves from the *start* of the status bar to the *right end*. Problem counts are now first.
  - (b) Per-cluster actions are drawn as a hover gear ("Cluster settings"), not a ⋯ menu with Settings / Connect or Disconnect / Remove. ⌘. isn't shown.
  - (c) The collapsed 76px sidebar shows a vertical scope label. Decision 18 says no cluster name or state, and decision 16 removed `kit::VerticalLabel`.
  - (d) The sidebar is 200px (the old resources column was 196px).
  - (e) ⌘0 is a new binding for All clusters.
- **Decision 20 (keyboard)**:
  - ⌘W is "close tab" in the design. Decision 20 has it close the inspector, then the dock, then the window.
  - ⌘\ is new.
  - ⌃Tab / ⌃⇧Tab cycle recent pages today. With tabs, VS Code convention would cycle tabs.
  - ⌘1 / ⌘2 focus the sidebar and the list today. In VS Code they focus editor groups, a possible collision with panes.
  - ⌘[ / ⌘] history: per tab or per window?
- **Decision 17 ⌥↵ (palette result in a new window)**: it may need a "new tab" / "split" counterpart.
- **Icon pipeline**: `mise-tasks/gen/icon.swift` scales the source 1.18× and clips it to a 824/1024 squircle. The new source is already a rounded tile with a baked drop shadow on opaque white, so the current task would double-mask it and keep white corners. The tile spans about 82% of the image.

## Open questions

1. Does a sidebar click replace the focused pane's active tab, or open a new tab? Are there preview tabs?
2. Does picking a cluster in the switcher retarget the active tab (same kind on the new cluster) or only the sidebar?
3. Max panes, vertical splits, pane resize, closing the last tab in a pane (does the pane close?), drag tabs between panes.
4. Tab overflow when the strip is full (scroll, shrink, or menu).
5. Persist tabs and panes per window across restarts?
6. Are logs now tabs as well as the Workload dock, or does the dock go away?
7. Does a split pane's compact deployment view keep any actions? What triggers compact vs full layout: pane count or width?
8. The tab cluster label is a short form (`us-east` for `prod-us-east`). What rule produces it?
9. The switcher's gear opens only Settings. Where do Connect / Disconnect and Remove from Flock live now?
10. Should cross-pane diff highlighting (drafted, disabled in `Split`) be built?
