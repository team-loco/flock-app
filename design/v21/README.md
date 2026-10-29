# Design v2.1 (experimental)

From the Oct 28 market and UX review. These boards amend `screens/` where they overlap; read them
the same way (markup for layout and styles, `<script>` for sample data and interactions). Replace
"Fleet" with Flock and "All clusters" scope wording as in `screens/`.

- `StatusBarMulti`, `StatusBarMultiOpen`: the status bar for a multi-cluster scope. Problem counts
  lead on the left (they open Problems), then the Kubernetes version when exactly one cluster is
  selected, then a spacer, Refresh, and the cluster selector at the right end. The selector shows
  "All clusters", or the first selected cluster with `+N`; its menu (300px, anchored right) has an
  All clusters checkbox (checked, mixed or empty, with "n of m"), a rule, one checkbox row per
  cluster (unavailable clusters in mute with their state as meta, gear on hover), a rule, then
  Add cluster…. At least one cluster stays selected.
- `QueryBar`: the filter becomes a query of chips. `@facet:value` adds a chip, `-@facet:value`
  excludes, free text matches names. Typing `@` lists facets with their value counts; `@facet:`
  lists that facet's values with counts and a hover "−" to exclude. Backspace on empty input
  removes the last chip, × clears, Esc closes the menu.
- `FleetBrowse`: Flock home gains a Resources section (Deployments, Pods, Nodes, Helm releases)
  listing objects across the selected clusters with a Cluster column, a query bar with facets
  (status, namespace, cluster, image), a "group" picker, collapsible group header rows with counts
  and ✕/▲ totals, checkbox selection with a bulk action bar, and an inspector with problems,
  Details, Other clusters (the same object elsewhere), Pods and Events.
- `LogsV3`: a Logs page: query bar with match position and previous/next, time-range buttons with
  counts, view toggles, Export; a collapsible Volume histogram stacked by level; a Sources panel
  (grouped, toggle per source, counts, Add source); the log with level, source colour, workload,
  pod/container and cluster per line and highlighted matches.
- `Nav`, `PaneTabs`: the sidebar and tab strip these boards import (`Nav mode=fleet exp` draws the
  Resources section).
