# Flock live scenarios

These fixtures create three local kind clusters, `fleet-a`, `fleet-b` and
`flock-loco`, with contexts `kind-fleet-a`, `kind-fleet-b` and `kind-flock-loco`
in the default kubeconfig. `flock-loco` also runs real Loco (see
[flock-loco](#flock-loco-real-loco-dogfood-cluster)). Each has
Flux and metrics-server, with `--kubelet-insecure-tls` for kind's kubelet
certificates. Docker must be running. Tool versions come from `mise.toml`.

```sh
mise trust && mise install
mise run e2e:up
mise run e2e:status
```

`up` reuses existing clusters, exports their contexts, and reapplies the
fixtures. Every operation selects its context explicitly. Cluster creation or
kubeconfig export can change the current context; Flock can select both named
contexts independently. Keep the clusters running for desktop testing.

Flux reconciles asynchronously. `up` runs `e2e/verify` after applying both
clusters. You can also run `./e2e/verify` independently. It waits for healthy
objects, expected failures, the suspended spec, metrics availability, and
pod/event evidence; it exits unsuccessfully if the expected states do not
appear. `status` prints
`flux get all -A` for each cluster. Allow several minutes for image downloads,
Helm installation timeouts, and crash backoff on the first run.

| Scenario | fleet-a | fleet-b | Flock screens |
| --- | --- | --- | --- |
| `HelmRelease/apps/podinfo` | Ready | Ready | Flock home, HelmRelease page, GitOps grid |
| `HelmRelease/payments/checkout-api` | Not Ready; pod crashes on `--flock-invalid-flag` | Ready; no extra arguments | Flock home, Workload logs/events, HelmRelease page, GitOps grid |
| `Deployment/payments/stripe-webhook` | CreateContainerConfigError | CreateContainerConfigError | Flock home, Workload environment/events |
| `HelmRelease/search/search-indexer` | Suspended | Suspended | HelmRelease page, GitOps grid |
| `Kustomization/flux-system/apps` | Ready; podinfo deployed into `web` | Ready; podinfo deployed into `web` | Kustomization page and inventory, Changes, GitOps grid |
| `Kustomization/flux-system/broken-kustomization` | ArtifactFailed; nonexistent path | ArtifactFailed; nonexistent path | Flock home, Kustomization page, GitOps grid |
| `Pod/apps/unschedulable` | Pending; FailedScheduling | Pending; FailedScheduling | Flock home, Workload/pod events |
| `Pod/apps/no-shell` | Running `pause` with no `/bin/sh` | — | Pods terminal: no-shell state and debug container |

The checkout release's only cluster-specific change is the extra invalid
argument in `scenarios/fleet-a/checkout-api.yaml`. The chart's `extraArgs` value
is appended to podinfo's command, so the process exits and Kubernetes restarts
it. Flux keeps the failed installation for inspection and retries without
uninstalling it. Both releases explicitly use the name `checkout-api`.

The dummy `stripe` secret contains `webhook-secret`, while `stripe-webhook`
requests `webhook_secret` through `STRIPE_WEBHOOK_SECRET` with `optional: false`.
This deliberately reproduces a configuration error at container creation;
Kubernetes cannot start the process, so this workload has events but no process
logs. The separate checkout scenario provides crash logs.

The public HelmRepository is `https://stefanprodan.github.io/podinfo`. The public
GitRepository tracks `master` at `https://github.com/stefanprodan/podinfo`.
The healthy Kustomization builds `./kustomize`; the broken one builds
`./flock-nonexistent-path`. Sources and generated HelmCharts should be Ready on
both clusters. The suspended release has no deployed workload or Ready
condition until resumed.

Changes can show the public Git source revision and upstream commit history.
Local scenario overlays are applied with kubectl and are not commits in that
source: this setup does not reproduce the design's fictional checkout commit
or its Git diff. GitOps grid comparison shows the differing Helm outcomes for
the same checkout chart and the matching healthy Git revision.

Upstream charts, Git branch, container tags, and metrics-server's release URL
follow their published defaults; an upstream change can alter the fixtures.
The verification script catches changes that break the expected states.

`mise run e2e:up` is safe to repeat on existing clusters. It sets up the three clusters in parallel,
each logging to its own file and printing that log's tail if it fails, then runs `e2e/verify` once.
Each cluster ends with `e2e/reset`, which deletes `Pod/apps/unschedulable` first when its node
selector predates the scenario (pod specs are immutable), reapplies the scenario manifests, uncordons
the control-plane node, reinstalls Loco on `flock-loco` and requests a Flux reconcile of every
Kustomization and HelmRelease.

`mise run e2e:test` is the one-command proof. It compiles both test builds in the background, runs
`e2e:up` (reusing existing clusters), or with `--skip-up` runs `e2e/reset` on each cluster and then
`e2e/verify` (which includes `web/podinfo`), so a leftover mutation cannot fail a test. Then it runs every `FLOCK_E2E` test with and without
Loco enabled, against a temporary kubeconfig that holds only
`kind-fleet-a`, `kind-fleet-b` and `kind-flock-loco`. Linux excludes the macOS app, headless renderer
and UI crates; the live backend harness runs in `flock-kube` on both platforms.
macOS also runs the headless UI tests. `--skip-up` requires existing clusters;
`--no-teardown` keeps them (the default, except when `CI` is set).

Tests that change shared scenario objects restore them even when they panic:
`cordon_then_uncordon_the_node` (the control-plane node) and the suspend journey below. Everything
else, including `scale_up_then_back_down`, uses its own scratch objects.

The live UI journeys in `apps/flock-shots/tests/journeys.rs` are part of that run. They open the
headless Flock shell on a `LiveBackend` with all three clusters added to a temporary data directory,
then visit every page, tab, inspector, row menu and shortcut and check that counts, badges and
pages stay loaded. Two of them change the clusters and put them back: one resumes and suspends
`HelmRelease/search/search-indexer` on fleet-a, then uninstalls what Flux installed on resume and
recreates the HelmRelease from its saved spec, so it is back to never-deployed (also when the test
fails); the
other creates and deletes the ConfigMap `default/flock-qa-live` on fleet-b. Render every live page
for review with
`FLOCK_E2E=1 mise exec -- cargo nextest run -p flock-shots --test journeys --run-ignored all live_pages_render_for_review`;
the PNGs land in `target/qa/`.

To remove both clusters when desktop testing is finished:

```sh
mise run e2e:down
```

This deletes only `fleet-a`, `fleet-b` and `flock-loco`, their kind kubeconfig entries and node
volumes, the kind node image when no other container uses it, and the Loco checkout in `target/loco`.

## flock-loco: real Loco dogfood cluster

`flock-loco` is created from `flock-loco/kind.yaml` (one node, `maxPods: 250`). It gets the same
metrics-server, Flux and `scenarios/common` objects as fleet-b (so `checkout-api` is Ready), plus
`scenarios/flock-loco/flux-states.yaml`, and then `flock-loco/install` installs Loco at `LOCO_REF`
from `mise.toml`:

- clones `team-loco/loco` at `LOCO_REF` into `target/loco/<ref>` for its charts and fixtures, and
  runs the published `ghcr.io/team-loco/loco-{api,controller,builder,agent}:sha-<commit>` images of
  that tag's commit (the node pulls them; nothing is built locally);
- applies Loco's `manifests/namespaces` and runs a Loco API in `loco-control-plane` the way Loco's own
  `e2e/control-plane.sh` does: Postgres (image from Loco's `compose.yaml`, ephemeral storage) and the
  `api-deployment` and `api-service` templates of `charts/loco-control-plane` with
  `flock-loco/control-plane-values.yaml`. The API starts on an empty database, runs its migrations
  and accepts the agent's `AGENT_TOKEN` as its connection secret;
- runs Loco's zot registry (image from Loco's `compose.yaml`, config and dev `htpasswd` from
  `registry/local`) and a source server in `loco-registry`, stores the builder push secret, and
  installs `charts/loco-operator` with `flock-loco/operator-values.yaml`;
- installs `charts/loco-core` as the Helm release `loco-core` with `flock-loco/core-values.yaml`:
  Envoy Gateway from its `gateway-helm` dependency, the `eg` Gateway, the chart's NetworkPolicies and
  the agent, which registers with the API and keeps its sync stream open;
- applies the workload generated by `loco.py`.

| Scenario | State | Flock screens |
| --- | --- | --- |
| 30 `ws-<id>` workspaces, 60 Applications (`web` + `api`) | 46 Ready with 1–2 replicas (Loco's validator forces a 100m CPU request per replica; `e2e/flock-loco/pod-cpu-policy.yaml` rewrites the pod request to 1m so the scenario fits a 4-vCPU CI node) and staged env Secrets | Applications, Workloads, Pods |
| `flock.dev/expect=Failed` Applications | 6 CrashLoopBackOff (missing health path), 3 ImagePullBackOff, 2 root image (CreateContainerConfigError) | Applications, Problems, Pods |
| `flock.dev/expect=Deploying` Applications | waiting for an env Secret revision that is never staged | Applications |
| `flock.dev/expect=Rollout` (`planet-express` web) | 4 replicas with a 180 s startup grace, re-rolled on every `up` and `e2e:test`, so a rollout is in progress for several minutes | Applications, Deployment |
| Builds in `loco-builds` | 5 Succeeded (Loco's `public-image` fixture pushed to zot), 2 Failed (missing Dockerfile, missing source), 1 Running (`RUN sleep`; build timeout 24 h) | Builds |
| HTTPRoutes on Gateway `eg` | 29 Accepted (the web app waiting for env Secrets has none yet); routes of failing web apps have no ready endpoints | Routes |
| `HelmRelease/ledger/ledger-api` | Not Ready; crashes on `--flock-invalid-flag` | HelmRelease page, GitOps grid |
| `HelmRelease/ledger/ledger-migrations` | Reconciling: install waits up to 1 h for a missing image tag | HelmRelease page |
| `Kustomization/flux-system/rollouts` | Reconciling: health check waits up to 1 h for a missing image tag | Kustomization page |
| `Kustomization/flux-system/frozen-kustomization` | Suspended | Kustomization page |
| `Job/reports/nightly-report-backfill` | Complete; 100 completed pods | Jobs, Pods |

Workspace, resource and build ids are stable UUIDv7-shaped values, so namespaces, pods and
hostnames are long. Running Loco components: `loco-controller`, `loco-build-controller`,
`envoy-gateway`, the `eg` Envoy proxy, `loco-core-agent`, and `loco-api` with `postgres` in
`loco-control-plane`. The agent talks to the API Service directly; the chart's `api-network-policy`
(Envoy-only ingress) and `api-route` are not applied, nor the dashboard or observability proxy.
The API has no source bucket or registry configured, so it never dispatches builds, and it ignores
the scenario's Applications and Builds because it has no placements or builds for them.

Loco's install contract, and what flock-loco does to fit one laptop or CI node:

- `charts/loco-core` needs `agent.clusterName`, `clusterRegion` and `clusterTier`, and a TLS Secret
  for the Gateway's HTTPS listeners. Without cert-manager, `certManager.existingTLSSecret` names a
  self-signed Secret that `install` creates once; the empty `global.cloudProvider.loadBalancerIPPool`
  default leaves out the Cilium pool. cert-manager, Cilium and the observability stack are not
  installed, so their Platform rows are missing, and the Envoy Service's LoadBalancer stays pending.
- Loco's validator requires at least 100m CPU and 32Mi per replica (requests = limits) and caps memory
  at 4Gi. `pod-cpu-policy.yaml` rewrites `ws-*` pod CPU requests to 1m so 46 healthy Applications fit
  a 4-vCPU CI node; there is no Application that is unschedulable, since it could not ask for more
  than 4Gi, and the 100-pod Job supplies the rest of the pods.
- Builds need a registry push secret, so `install` runs zot and the source server, stores
  `loco-registry-push` and adds the pod CIDR to `builds.privateEgressCIDRs`.
- The agent removes finished builds after `buildRetention` (1h by default); it is raised to 8760h.
- The API cancels any unfinished Build the agent reports that it did not dispatch, and the agent
  reports Builds named `build-<buildId>`. The scenario's Builds are named `flock-build-<buildId>`, so
  the Running build stays running.

`e2e/verify` checks the Loco controllers, Envoy Gateway, the API, Postgres, that the agent is Ready
and its log shows the sync stream connected, the Gateway, every `flock.dev/expect` phase on
Applications and Builds, that every HTTPRoute is Accepted, the Flux states above, and (with
`loco.py --check`) at least 200 pods, a crashlooping workspace pod and a rollout that is Deploying or
Ready. The generator's tests run with
`mise exec -- python3 -m unittest discover -s e2e -p test_loco.py`.

With `--features loco`, `live_every_loco_page_loads_real_loco_with_counts_and_no_toasts` in
`journeys.rs` visits Platform, Applications, Builds and Routes on flock-loco twice, waits for their
rows and counts, selects the first row, and checks for skeletons, error states and toasts.

## Scale and snapshot performance

`mise run e2e:scale` adds a disposable `flock-scale` namespace to **kind-fleet-b only**.
The task obtains a temporary kubeconfig directly from kind, uses an explicit context on every
kubectl call, and never loads the user's kubeconfig. It creates 3,000 ConfigMaps with 1 KiB payloads,
1,500 headless Services, 400 zero-replica Deployments (and their controller-created ReplicaSets),
2,000 Events and 200 pause Pods. Pods above the node's pod capacity can remain Pending; they still
exercise the real pod watcher. Eight bounded apply workers keep setup practical. Re-running is
idempotent. The image is pinned in `mise.toml`.

Run `mise run e2e:scale:bench` after setup. The ignored release tests measure selected-cluster page
rebuilds, live heap freed when watcher stores and tasks are released, and pod label patches through
the real LiveBackend channel into the UI's `AppState`. Twelve isolated probes, one second apart, must
reach `AppState` with a median under 300 ms including the API request; their maximum after the API
acknowledged the patch must stay under 1 s, so a stalled API server on a loaded host does not fail
the run. A ten-second storm of eight concurrent pod patchers then checks that snapshots stay at most
one per snapshot interval while probes still get through. Each line prints the load average. A separate
heap test starts the LiveBackend on kind-fleet-b and reports the time to the first Pods and
Deployments counts after connect, then the process heap and the API server's watch count (the sum of
`apiserver_longrunning_requests` with `verb="WATCH"`, minus the count before Flock connected) after
connect and on the ConfigMaps page. This
measures state availability, not a rendered gpui frame. The API server drops Events after its one-hour
event TTL, so re-run `mise run e2e:scale` before benchmarking an older scenario. The heap test uses [dhat](https://docs.rs/dhat/latest/dhat/) in its own profiling scope;
timing measurements run without profiling. RSS is also reported, but includes clients and runtime
memory and is not used as a store heap measurement. Run these tests alone to avoid CPU contention.

`mise run bench:snapshot` needs no cluster. It builds 10,000 realistic pod view models and their
overview index, plus 5,000 metadata-only ConfigMap rows. It reports medians and maxima over 40 samples
and enforces a 20 ms median rebuild budget. Allocation budgets catch temporary string cloning even
when wall-clock measurements are noisy. Normal `mise run check` leaves these timing tests ignored.
The generator's boundary tests run with
`mise exec -- python3 -m unittest discover -s e2e -p test_scale.py`.

Always finish with `mise run e2e:scale:down`, then `e2e/verify` using a temporary kubeconfig containing
only the two kind contexts. Teardown deletes only `flock-scale` on kind-fleet-b and preserves the
normal Flux scenarios. Benchmark setup fails if kind-fleet-b or the scale objects are missing.
