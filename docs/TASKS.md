# Task ledger

Claim a row (owner + branch, status `doing`) before starting. Status: `todo` · `doing` · `pr` · `done`.
Spec column points at `design/`.

## Phase 0 — scaffold

| id | unit | spec | owner | branch | status |
|---|---|---|---|---|---|
| P0 | workspace, mise tasks, lefthook, CI, flock-core contract, theme tokens, window shell (strip, two panels, page header, status bar) on demo data | HANDOFF.md, Nav, StatusBar | claude | p0-scaffold | done (#2) |

## Phase 1 — foundations (parallel)

| id | unit | spec | owner | branch | status |
|---|---|---|---|---|---|
| F1 | ui-kit: status glyph, section label, rule, buttons, ⋯ action menu with descriptions, table on uniform_list, dock, collapsible panel animation, code/diff view | HANDOFF visual rules | claude | f1-ui-kit, f1-ui-kit-2 | done (#3, #12) |
| F2 | flock-kube: kubeconfig discovery + watch, login-shell env capture, cluster connect + checks, per-cluster watchers → core snapshots | AddCluster, AddClusterCheck | claude | f2-flock-kube, f2-flock-kube-live | done (#7, #9) |
| F3 | flock-flux: CRD types (kopium, Flux 2.9.6), status summarizer, inventory, Helm storage decode, cross-cluster matrix | Kustomization, Flux | Codex | f3-flock-flux | done |
| F4 | flock-loco: CRD types generated from the pinned Loco release, app/build/route/platform models | Loco* | Codex | f4-flock-loco | done |
| F5 | e2e: kind clusters + Flux + scenario workloads (crashloop, missing secret key, failing HelmRelease, suspended, drift) | Flock, Workload | codex | f5-e2e-scenarios | done (#1) |
| F6 | folded into screen units: each S-unit owns its flock-core model, flock-demo data and flock-ui screen | | | | dropped |

## Phase 2 — screens (parallel)

| id | unit | spec | owner | branch | status |
|---|---|---|---|---|---|
| S1 | Flock home: clusters table, problems by cluster, recent GitOps commits | Flock | claude | s1-flock-home | done |
| S2 | Command palette ⌘K | Command, v1/Palette | claude | s2-command-palette | done |
| S3 | Workload (Deployment) page + logs / events / terminal dock | Workload | claude | s3-workload | done |
| S4 | Flux object page (Kustomization + HelmRelease + sources) | Kustomization | claude | s4-flux-object | done |
| S5 | Changes: commits, per-cluster outcome, full diff, rendered views | Changes | claude | s5-changes | done |
| S6 | GitOps sync grid across clusters | Flux | claude | s6-gitops-grid | done |
| S7 | Add cluster flow (contexts, checks) | AddCluster, AddClusterCheck | claude | s7-add-cluster, s7-add-cluster-live | done (#18) |
| S8 | Cluster overview, Nodes, Namespaces, Events, generic resource lists | v1/Main, Nav | claude | s8-cluster-pages | done (#17, overview) |
| S9 | Pods: list, details, logs, shell | v1/Pods | claude | s9-pods | done |
| S10 | Compare across clusters, cluster settings (Catalog) | v1/Compare, v1/Catalog | claude | s10-compare-settings | done |
| S11 | Loco: Platform, Applications, Builds, Routes | Loco* | claude | s11-loco | done (#25, Platform + Routes PR) |

## Phase 3 — integration

| id | unit | owner | branch | status |
|---|---|---|---|---|
| I1 | live backend wired into every screen (I1a: home, cluster pages, pods; I1b: live Flux object pages, GitOps grid, Changes + home commits from git mirrors, reconcile progress; I1d: live Loco Platform, Applications, Builds, Routes, restart, cancel build) | claude | i1a-live-cluster-pages, i1b-live-flux, i1d-live-loco | done (#30, #32, #33, #36) |
| I1c | live follow-ups: startup presets on live, YAML refresh, connection commands, one Flux summary, terminal resize, StatefulSet/DaemonSet rollback, live Compare | claude | i1c-presets-yaml, i1c-connection, i1c-flux-summary, i1c-terminal-rollback, i1c-compare-live | done (#81, #88, #92, #101, compare PR) |
| I2 | actions: reconcile, suspend/resume, scale, restart, delete, port-forward, exec, with confirmation | claude | i2-live-actions | done (#29, #31) |
| I3 | e2e run against kind scenarios | claude | i3-e2e-suite | done |
| I4 | Flock.app bundle, icon (squircle 22%), version stamping | claude | i4-app-bundle | done |
| I5 | animation polish + parity pass, dark and light, every screen | | | todo |
| I6 | headless screenshots (`mise run shot`) and UI interaction test helper (`flock_shots::Ui`), no on-screen window | claude | headless-shots | done |
| CPERF | scale scenarios, snapshot benchmarks and performance fixes | claude | cperf-scale | done |

## Font repair

| id | unit | owner | branch | status |
|---|---|---|---|---|
| GLYPH | restore upstream Plex and ship Flock Status glyph font (#95) | Codex | status-glyph-font | done |
| GLYPH2 | bundle the all-clusters bullseye in Flock Status | Codex | all-clusters-glyph | done (#100) |

## Follow-ups

| id | unit | owner | branch | status |
|---|---|---|---|---|
| CXD | Events namespace filter, full messages and inspector (#131) | claude | events-inspector | done |
| C77 | Palette open in new window; split follow-up in #77 | codex | c77-windows-split | done |
| CXA | Flux list pages; per-page watch state with loading, empty and error states (#124, #123) | codex, claude | loading-states | done |
| SEC1 | redact Secret data in YAML views and Helm diffs; strip credentials from URLs, git errors, auth and kubeconfig errors | claude | secret-redaction | done |
| SHELL2 | single sidebar, cluster switcher in the status bar, Loco nav only where installed, animated inspectors (#110, #111, #121) | claude | shell-single-sidebar | done |
| PIO | Independent network calls run concurrently (git mirrors, checks, Flux/Loco probes, workload page, drain, Prometheus); bounded git output (#107, #109) | claude | parallel-io | done |
| SET1 | app settings ⌘, : theme, data refresh, notifications (#112) | claude | settings-and-keys | done |
| TBL1 | resizable table columns remembered per cluster, one FilterBar on every list, popovers occlude hover (#115, #116, #118) | claude | tables-and-filters | done |
| KEYS1 | VS Code-familiar bindings, one binding table, history, full keyboard navigation (#114) | claude | keyboard | done |
| SAFE1 | confirmed mutations carry the UID of the object the user saw; drain evicts by UID (#103, #104) | claude | mutation-safety | done |
| CXE | resizable inspectors and scrollable YAML (#133) | claude | inspector-resize | done |
| SAFE2 | cluster identity pinned (server + CA), Context changed prompt, disconnect tears down forwards/streams/supervisor, credential/CA rotation reconnects (#105, #106, #108) | claude | cluster-identity | done |
| CXC | Overview resource visuals and navigation; full nav selection (#125, #126, #122) | claude | overview-visuals | done |
| CXB | Repair terminal TTY, rendering and explicit shell lifecycle; remove previous container logs (#128, #129) | claude | terminal-fix | done |
| TBL2b | one namespace memory per cluster and list in cluster preferences; pods and resource lists scoped again after #146 (#117) | claude | tables-namespace-unify | done |
| TBL3 | actions on table rows (hover ⋯, right-click), inspectors keep the primary action; no summary sentence on list pages (#119, #120) | claude | tables-row-actions-2 | done |
| SAFE3 | Compare apply-spec and Loco cancel-build/restart carry the UID the user saw; Compare follows clusters that connect after it opens (#141, #136) | claude | compare-safety | done |
| UPD1 | release pipeline: `mise run release`, tag-triggered arm64 build, ed25519-signed zip on GitHub Releases | claude | auto-update | done |
| UPD2 | in-app updater: check, verify, stage, restart to update; menu, palette, settings and status bar | claude | auto-update-app | done |
| TABS1 | pane tabs (one pane), v3 sidebar and status-bar switcher, tabs persisted per window | claude | tabs-and-shell-v3, tabs-shell-v3-nav, tabs-persist | done (#158, #160, persistence PR) |
| HL1 | primary and secondary highlight colours: presets or hex, derived for dark and light with WCAG checks | claude | themes | done |
| NAME1 | rename the product to Flock: crates, binary, bundle id, env vars, docs; move the old data and cache dirs on launch | claude | rename-to-flock | done |
| C167 | share multi-file kubeconfig isolation across E2E harnesses (#167) | codex | fix-167 | done |
| HDR1 | one 32px title row with the traffic lights; collapsed sidebar hidden, lights and chevron lead the tab strip; singular node count (#165) | claude | header-traffic-lights | done |
| SPLIT1 | split right: pane-owned screens and backend scopes, two panes, compact pane views (#77) | claude | split-panes | done (#170, split UI PR) |
| SEL1 | selectable, copyable text: kit `Selectable`/`SelectableText` on the window text selection, ⌘C/⌘A, terminal drag-select (#127) | claude | selectable-text | done (#173, adoption PR) |
| POL | Global shortcut focus recovery, selection without outlines, subtle skeletons, editable default namespace picker (#179–#182) | codex | polish-focus-skeleton | done |
| BG1 | every nav kind watched in the background; nav counts and badges for every cluster at all times; bounded event window (#177) | claude | background-watches | done |
| BG2 | pages read the background stores, namespace picker filters the view, no skeleton when data is in memory (#177, #178) | claude | pages-from-stores | done |
| EMP1 | list pages keep the filter bar in every state; empty states name what hides the rows and offer Show all namespaces or Clear filter (#190) | claude | empty-state-filters | done |
| QFB | list-page QA fixes: on-demand section states, e2e:up across the rename, content-sized columns, Pods dock empty state (#204-#207) | claude | qa-fixes-lists | done |
| QAF1 | QA Flux fixes: failing objects stay failing through retries, real suspend age, Resume as primary when suspended, short names keep whole names, diff hunk headers (#200–#203, #208) | claude | qa-fixes-flux | done |
| NAVBTN | visible per-pane back/forward controls, history menus and mouse navigation (#191) | claude | back-forward-buttons | done |
| CHART | Persist seven days of utilization history and render every chart window (#193) | claude | utilization-history | done |
| SETTINGS | Open Cluster settings on the cluster each link came from (#219) | claude | settings-cluster-context | done |
| FILTER | Clear filter placeholder, namespace/label completion and removable tokens (#189) | claude | filter-bar-ux | done |
| C185 | Linux e2e coverage and restart rollback revision selection (#185, #186) | codex | fix-ci-e2e-rollback | done (#195) |
| MLOG | multi-pod log streaming: merged, colour-coded, legend toggles, pause, pods join and leave (#230) | claude | multi-pod-logs | done |
| EDIT1 | Edit YAML: editor tab, dry-run server-side diff, UID/resourceVersion-pinned apply with conflict rebase, Secrets redacted unless revealed (#231) | claude | yaml-edit-apply | done |
| WHY1 | Reason column on every list; inspectors lead with what's wrong, conditions and events; per-kind inspector widths (#239) | claude | list-reasons-inspectors | done |
| DOG1 | flock-loco kind cluster with real Loco, Flux states and ~30 workspaces; verify and Loco live journeys (#229) | claude | dogfood-loco-cluster | done |
| MENUBAR | Menu-bar health, problem links and persisted setting (#233) | claude | menu-bar-status | done |
| REL1 | detail pages for every kind, Namespace page, Related section in every inspector (#240) | claude | drilldown-detail-pages | done |
| NS1 | namespace picker lists every namespace from the Namespace watch, with per-kind counts (#241) | claude | ns-picker-fixes | done |
| FLUX1 | Flux dependency graph with cross-cluster comparison, live Flux events, Kustomization drift (#232) | claude | flux-graph-events-drift, flux-events-stream, kustomization-drift | done (#259, #262, drift PR) |
| TERM2 | terminal reflows long lines on resize; no-shell exec offers a debug container; distroless e2e pod (#242) | claude | terminal-wrap-no-shell | done |
| E2E250 | flock-kube live tests create and delete their own scratch namespace; default stays empty (#250) | claude | action-tests-namespace | done |
| QA22 | v0.2.2 dogfood QA: page shortcuts before focus, ⌘E Edit YAML, Flux events keyboard, Edit YAML drafts across tabs (#268–#271) | claude | fix-keyboard-focus, fix-edit-draft-tabs | done |
| CRD1 | CRD browser: Custom resources sidebar, printer columns, inspector, Edit YAML; Helm releases from Helm storage (#234) | claude | crd-browser-helm, helm-releases-pr | done (#275, Helm PR) |
| HIST1 | history entries and saved tabs carry the inspected object, so Back and Forward reopen a list's inspector (#281) | claude | history-keeps-inspector | done |
| CHART298 | Utilization chart: centred empty state without grid or axes, wording says why the window is empty, unclipped x labels (#298) | claude | utilization-empty-state | done |
