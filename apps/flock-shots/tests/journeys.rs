use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use flock::{Preset, Screen};
use flock_core::app_settings::Highlight;
use flock_core::history::Location;
use flock_core::{
    AppState, CLUSTER_NAV, ClusterId, ContextRef, FluxKey, FluxKind, NewCluster, Page, ResourceRef,
    Status, WatchState,
};
use flock_kube::LiveBackend;
use flock_kube::discovery::{DiscoverySources, discover};
use flock_kube::settings::KubeSettings;
use flock_kube::store::ClusterStore;
use flock_shots::{BackendFactory, Highlights, LiveShots, Ui};
use flock_ui::theme::Appearance;

#[path = "../../../crates/flock-kube/tests/common/kubeconfig.rs"]
mod kubeconfig;

const E2E_VAR: &str = "FLOCK_E2E";
const CLUSTERS: [(&str, &str); 3] = [
    ("fleet-a", "kind-fleet-a"),
    ("fleet-b", "kind-fleet-b"),
    ("flock-loco", "kind-flock-loco"),
];
const DATA_DIR: &str = "data";
const LOCO_SECTION: &str = "Loco";
const CONNECT: Duration = Duration::from_secs(120);
const PAGE_TIMEOUT: Duration = Duration::from_secs(45);
const SHOTS_DIR: &str = "../../target/qa";
const FLOCK_PAGES: [Page; 6] = [
    Page::Overview,
    Page::Problems,
    Page::GitOps,
    Page::Changes,
    Page::Settings,
    Page::ClusterSettings,
];
const FLUX_OBJECTS: [(FluxKind, &str, &str); 7] = [
    (FluxKind::HelmRelease, "apps", "podinfo"),
    (FluxKind::HelmRelease, "payments", "checkout-api"),
    (FluxKind::HelmRelease, "search", "search-indexer"),
    (FluxKind::Kustomization, "flux-system", "apps"),
    (
        FluxKind::Kustomization,
        "flux-system",
        "broken-kustomization",
    ),
    (FluxKind::GitRepository, "flux-system", "podinfo"),
    (FluxKind::HelmRepository, "flux-system", "podinfo"),
];
const DEPLOYMENTS: [(&str, &str); 3] = [
    ("payments", "checkout-api"),
    ("payments", "stripe-webhook"),
    ("web", "podinfo"),
];

struct Kind {
    home: tempfile::TempDir,
}

impl Kind {
    fn data(&self) -> PathBuf {
        self.home.path().join(DATA_DIR)
    }

    fn factory(self) -> BackendFactory {
        let kind = Rc::new(self);
        Rc::new(move || {
            let sources = DiscoverySources {
                kubeconfig_env: None,
                home: Some(kind.home.path().to_path_buf()),
                folders: Vec::new(),
            };
            let store = ClusterStore::open(&kind.data())?;
            let backend = LiveBackend::new(KubeSettings::default(), sources, store);
            Ok(Box::new(backend))
        })
    }
}

fn kind() -> anyhow::Result<Option<BackendFactory>> {
    if std::env::var(E2E_VAR).as_deref() != Ok("1") {
        return Ok(None);
    }
    let sources = DiscoverySources {
        kubeconfig_env: std::env::var_os("KUBECONFIG"),
        home: None,
        folders: Vec::new(),
    };
    let discovery = discover(&sources);
    let contexts = CLUSTERS
        .iter()
        .map(|(_, name)| {
            discovery
                .contexts
                .iter()
                .find(|context| context.id.context == *name)
                .cloned()
                .with_context(|| format!("KUBECONFIG must define {name}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let kind = Kind {
        home: kubeconfig::kind_only_home(&contexts),
    };
    add_clusters(kind.home.path(), &kind.data())?;
    Ok(Some(kind.factory()))
}

fn add_clusters(home: &Path, data: &Path) -> anyhow::Result<()> {
    let file = home.join(".kube").join("config");
    let mut store = ClusterStore::empty(data);
    let added = CLUSTERS
        .iter()
        .map(|(name, context)| NewCluster {
            context: ContextRef {
                file: file.clone(),
                context: (*context).to_owned(),
            },
            name: (*name).to_owned(),
        })
        .collect::<Vec<_>>();
    store.add(&added)?;
    Ok(())
}

fn connected(state: &AppState) -> bool {
    CLUSTERS.iter().all(|(name, _)| {
        state
            .snapshot
            .cluster(&ClusterId::new(*name))
            .is_some_and(|cluster| cluster.is_connected() && cluster.status != Status::Progress)
    })
}

fn open(screen: Screen, make: &BackendFactory) -> anyhow::Result<Ui> {
    let mut ui = Ui::live(screen, Preset::default(), make.clone())?;
    ui.wait_until(CONNECT, connected)?;
    Ok(ui)
}

#[test]
fn live_flock_home_connects_both_kind_clusters() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let ui = open(Screen::Flock, &make)?;
    assert_eq!(ui.state().location().page, Page::Overview);
    Ok(())
}

fn flux_loaded(state: &AppState) -> bool {
    CLUSTERS.iter().all(|(name, _)| {
        state
            .snapshot
            .flux
            .iter()
            .any(|object| object.key.cluster.as_str() == *name)
    })
}

fn page_ready(state: &AppState, location: &Location) -> bool {
    let Some(cluster) = &location.cluster else {
        return connected(state) && flux_loaded(state);
    };
    let watch = state
        .snapshot
        .pages
        .get(cluster)
        .and_then(|pages| pages.get(&location.page));
    connected(state) && flux_loaded(state) && !matches!(watch, Some(WatchState::Pending(_)))
}

fn cluster_pages() -> impl Iterator<Item = Page> {
    CLUSTER_NAV
        .iter()
        .filter(|section| section.label != Some(LOCO_SECTION))
        .flat_map(|section| section.pages.iter().copied())
}

fn review_locations() -> Vec<Location> {
    let mut locations: Vec<Location> = FLOCK_PAGES.into_iter().map(Location::flock).collect();
    for (name, _) in CLUSTERS {
        let cluster = ClusterId::new(name);
        locations.extend(cluster_pages().map(|page| Location::page_on(cluster.clone(), page)));
        for (kind, namespace, object) in FLUX_OBJECTS {
            locations.push(Location {
                flux_object: Some(FluxKey::new(name, kind, namespace, object)),
                ..Location::page_on(cluster.clone(), kind.page())
            });
        }
        for (namespace, object) in DEPLOYMENTS {
            let target = ResourceRef {
                cluster: cluster.clone(),
                kind: "Deployment".into(),
                namespace: Some(namespace.into()),
                name: object.into(),
            };
            locations.push(Location {
                resource: Some(target.clone()),
                ..Location::page_on(cluster.clone(), Page::Deployments)
            });
            locations.push(Location {
                resource: Some(target),
                ..Location::page_on(cluster.clone(), Page::Compare)
            });
        }
    }
    locations
}

fn shot_name(location: &Location) -> String {
    let scope = location.cluster.as_ref().map_or("flock", ClusterId::as_str);
    let page = location.page.label().to_lowercase().replace(' ', "-");
    let object = location
        .flux_object
        .as_ref()
        .map(|key| format!("-{}-{}", key.namespace, key.name))
        .or_else(|| {
            location
                .resource
                .as_ref()
                .map(|target| format!("-{}", target.name))
        })
        .unwrap_or_default();
    format!("{scope}-{page}{object}")
}

#[test]
#[ignore = "renders every live page for review into target/qa; run with FLOCK_E2E=1 --run-ignored"]
fn live_pages_render_for_review() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(SHOTS_DIR);
    let locations = review_locations();
    let variants = [
        ("dark", Appearance::Dark, Highlights::default()),
        ("light", Appearance::Light, Highlights::default()),
        (
            "dark-teal-orange",
            Appearance::Dark,
            Highlights {
                primary: Highlight::Teal,
                secondary: Highlight::Orange,
            },
        ),
    ];
    for (label, appearance, highlights) in variants {
        let shots = LiveShots {
            appearance,
            highlights,
            timeout: PAGE_TIMEOUT,
            ready: &page_ready,
        };
        flock_shots::render_live(&make, &shots, &locations, |location, image| {
            let path = dir.join(format!("{}-{label}.png", shot_name(location)));
            flock_shots::save(&image, &path)
        })?;
    }
    Ok(())
}

fn selector(text: String) -> &'static str {
    text.leak()
}

fn nav(page: Page) -> &'static str {
    selector(format!("nav-{}", page.label().to_lowercase()))
}

fn ready_here(state: &AppState) -> bool {
    page_ready(state, state.location())
}

fn switch_to(ui: &mut Ui, cluster: &str) -> anyhow::Result<()> {
    ui.press("cmd-shift-c")?;
    ui.type_text(cluster)?;
    ui.press("enter")?;
    let id = ClusterId::new(cluster);
    anyhow::ensure!(
        ui.state().location().cluster.as_ref() == Some(&id),
        "the switcher did not move to {cluster}"
    );
    ui.wait_until(PAGE_TIMEOUT, ready_here)
}

fn visit(ui: &mut Ui, page: Page) -> anyhow::Result<()> {
    ui.click(nav(page))
        .with_context(|| format!("visiting {page:?} from {:?}", ui.state().location()))?;
    anyhow::ensure!(
        ui.state().location().page == page,
        "clicking {page:?} in the sidebar went to {:?}",
        ui.state().location().page
    );
    ui.wait_until(PAGE_TIMEOUT, ready_here)
        .with_context(|| format!("{page:?} stayed loading"))
}

fn counted(state: &AppState) -> Vec<(String, Page)> {
    state
        .snapshot
        .clusters
        .iter()
        .flat_map(|cluster| {
            cluster
                .nav
                .iter()
                .filter(|(_, counts)| counts.total.is_some())
                .map(|(page, _)| (cluster.name.clone(), *page))
        })
        .collect()
}

fn all_counted(state: &AppState) -> bool {
    CLUSTERS.iter().all(|(name, _)| {
        state
            .snapshot
            .cluster(&ClusterId::new(*name))
            .is_some_and(|cluster| {
                BACKGROUND_PAGES
                    .iter()
                    .all(|page| cluster.nav.get(page).is_some_and(|c| c.total.is_some()))
            })
    })
}

const BACKGROUND_PAGES: [Page; 16] = [
    Page::Nodes,
    Page::Namespaces,
    Page::Pods,
    Page::Deployments,
    Page::StatefulSets,
    Page::DaemonSets,
    Page::Jobs,
    Page::CronJobs,
    Page::Kustomizations,
    Page::HelmReleases,
    Page::Sources,
    Page::Services,
    Page::Ingresses,
    Page::ConfigMaps,
    Page::Secrets,
    Page::Pvcs,
];

const ERROR_BADGES: [Page; 2] = [Page::Pods, Page::Deployments];

fn missing_counts(before: &[(String, Page)], state: &AppState) -> Vec<String> {
    let now = counted(state);
    before
        .iter()
        .filter(|entry| !now.contains(entry))
        .map(|(cluster, page)| format!("{cluster}/{page:?}"))
        .collect()
}

fn missing_error_badges(state: &AppState) -> Vec<String> {
    let mut missing = Vec::new();
    for cluster in &state.snapshot.clusters {
        for page in ERROR_BADGES {
            if cluster.nav.get(&page).is_none_or(|counts| counts.err == 0) {
                missing.push(format!("{}/{page:?}", cluster.name));
            }
        }
    }
    missing
}

fn toasts(ui: &Ui) -> Vec<String> {
    ui.read_app(flock_ui::kit::toast_titles)
        .into_iter()
        .map(|title| title.to_string())
        .collect()
}

fn check_invariants(ui: &mut Ui, before: &[(String, Page)], step: &str) -> anyhow::Result<()> {
    let state = ui.state();
    let lost = missing_counts(before, &state);
    anyhow::ensure!(
        lost.is_empty(),
        "after {step}: nav counts disappeared for {lost:?}"
    );
    let badges = missing_error_badges(&state);
    anyhow::ensure!(
        badges.is_empty(),
        "after {step}: error badges disappeared for {badges:?}"
    );
    let shown = toasts(ui);
    anyhow::ensure!(
        shown.is_empty(),
        "after {step}: unprovoked toasts {shown:?}"
    );
    check_layout(ui, step)
}

fn check_layout(ui: &mut Ui, step: &str) -> anyhow::Result<()> {
    let window = gpui::px(flock::WINDOW_WIDTH);
    let sidebar = ui.bounds("sidebar");
    let pages = ui
        .state()
        .snapshot
        .nav_sections(ui.state().location().cluster.as_ref())
        .into_iter()
        .flat_map(|section| section.pages.iter().copied())
        .collect::<Vec<_>>();
    if let Some(sidebar) = sidebar.filter(|bounds| bounds.size.width > gpui::Pixels::ZERO) {
        for page in pages {
            let Some(row) = ui.bounds(nav(page)) else {
                continue;
            };
            anyhow::ensure!(
                row.right() <= sidebar.right() && row.left() >= sidebar.left(),
                "after {step}: the {page:?} nav row {row:?} overflows the sidebar {sidebar:?}"
            );
        }
    }
    let switcher = ui
        .bounds("cluster-switcher")
        .context("the cluster switcher was not painted")?;
    anyhow::ensure!(
        switcher.right() <= window,
        "after {step}: the cluster switcher {switcher:?} overflows the {window:?} window"
    );
    Ok(())
}

fn opened(make: &BackendFactory, screen: Screen) -> anyhow::Result<(Ui, Vec<(String, Page)>)> {
    let mut ui = open(screen, make)?;
    ui.wait_until(CONNECT, |state| flux_loaded(state) && all_counted(state))
        .context("every background kind should be counted on both clusters")?;
    let before = counted(&ui.state());
    Ok((ui, before))
}

#[test]
fn live_every_sidebar_entry_loads_and_keeps_counts_and_badges() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    for page in [Page::Overview, Page::Problems, Page::GitOps, Page::Changes] {
        visit(&mut ui, page)?;
        check_invariants(&mut ui, &before, &format!("visiting {page:?}"))?;
    }
    for (name, _) in CLUSTERS {
        switch_to(&mut ui, name)?;
        for round in ["first visit", "return"] {
            for page in cluster_pages() {
                visit(&mut ui, page)?;
                let step = format!("{round} to {name} {page:?}");
                anyhow::ensure!(
                    ui.bounds("error-state").is_none(),
                    "{step} shows an error state"
                );
                check_invariants(&mut ui, &before, &step)?;
            }
        }
    }
    Ok(())
}

fn skeletons_going_back(ui: &mut Ui) -> anyhow::Result<Vec<Page>> {
    let mut skeletons = Vec::new();
    for page in cluster_pages().skip(1) {
        ui.click(nav(page))?;
        ui.wait_until(PAGE_TIMEOUT, ready_here)?;
        ui.press("cmd-[")?;
        if ui.bounds("loading-skeleton").is_some() {
            skeletons.push(ui.state().location().page);
        }
        ui.press("cmd-]")?;
    }
    Ok(skeletons)
}

fn skeletons_switching_tabs(ui: &mut Ui) -> anyhow::Result<Vec<Page>> {
    for page in cluster_pages().skip(1) {
        ui.cmd_click(nav(page))?;
    }
    let count = ui.state().panes().focused().tabs().len();
    for _ in 0..count {
        ui.press("ctrl-tab")?;
        ui.wait_until(PAGE_TIMEOUT, ready_here)?;
    }
    let mut skeletons = Vec::new();
    for _ in 0..count {
        ui.press("ctrl-tab")?;
        if ui.bounds("loading-skeleton").is_some() {
            skeletons.push(ui.state().location().page);
        }
    }
    Ok(skeletons)
}

#[test]
fn live_returning_to_a_page_or_tab_never_shows_a_skeleton() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, _) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, CLUSTERS[0].0)?;
    let history = skeletons_going_back(&mut ui)?;
    anyhow::ensure!(
        history.is_empty(),
        "going back showed a skeleton on {history:?}"
    );
    let tabs = skeletons_switching_tabs(&mut ui)?;
    anyhow::ensure!(
        tabs.is_empty(),
        "switching tabs showed a skeleton on {tabs:?}"
    );
    Ok(())
}

#[test]
fn live_flux_failures_stay_flagged_for_two_minutes() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, _) = opened(&make, Screen::Flock)?;
    let flagged = |state: &AppState| {
        CLUSTERS.iter().all(|(name, _)| {
            state.snapshot.home.problems.iter().any(|entry| {
                entry.target.cluster.as_str() == *name
                    && entry.target.name == "broken-kustomization"
            })
        })
    };
    ui.wait_until(CONNECT, flagged)?;
    let start = std::time::Instant::now();
    while start.elapsed() < FLAP_WINDOW {
        ui.pause(Duration::from_secs(1))?;
        anyhow::ensure!(
            flagged(&ui.state()),
            "broken-kustomization left Problems after {:?}",
            start.elapsed()
        );
    }
    Ok(())
}

const FLAP_WINDOW: Duration = Duration::from_secs(120);

fn tab_count(ui: &Ui) -> usize {
    ui.state().panes().focused().tabs().len()
}

#[test]
fn live_tabs_open_per_page_switch_split_and_close() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, CLUSTERS[0].0)?;
    for page in cluster_pages().skip(1) {
        ui.cmd_click(nav(page))?;
    }
    let pages = cluster_pages().count();
    anyhow::ensure!(
        tab_count(&ui) == pages,
        "expected one tab per page, got {:?}",
        ui.state()
            .panes()
            .focused()
            .tabs()
            .iter()
            .map(|t| t.location().page)
            .collect::<Vec<_>>()
    );
    for ix in 0..pages {
        ui.press("ctrl-tab")?;
        ui.wait_until(PAGE_TIMEOUT, ready_here)?;
        check_invariants(&mut ui, &before, &format!("switching to tab {ix}"))?;
    }
    for ix in 0..pages {
        let strip = selector(format!("tab-{ix}"));
        anyhow::ensure!(
            ui.bounds(strip).is_some() || ix > 0,
            "tab {ix} has no strip entry"
        );
    }
    ui.press("cmd-\\")?;
    anyhow::ensure!(ui.state().panes().panes().len() == 2, "⌘\\ did not split");
    ui.wait_until(PAGE_TIMEOUT, ready_here)?;
    check_invariants(&mut ui, &before, "splitting")?;
    switch_to(&mut ui, CLUSTERS[1].0)?;
    check_invariants(&mut ui, &before, "switching the split pane's cluster")?;
    while ui.state().panes().panes().len() > 1 || tab_count(&ui) > 1 {
        let tabs = ui
            .state()
            .panes()
            .panes()
            .iter()
            .map(|p| p.tabs().len())
            .sum::<usize>();
        ui.press("cmd-w")?;
        let after = ui
            .state()
            .panes()
            .panes()
            .iter()
            .map(|p| p.tabs().len())
            .sum::<usize>();
        anyhow::ensure!(after < tabs, "⌘W did not close a tab ({tabs} tabs)");
    }
    ui.press("cmd-w")?;
    anyhow::ensure!(
        ui.state().location().page == Page::Overview && ui.state().location().cluster.is_none(),
        "closing the last tab should leave Flock home"
    );
    check_invariants(&mut ui, &before, "closing every tab")
}

fn sidebar_round_trip(ui: &mut Ui, from: &str) -> anyhow::Result<()> {
    ui.press("cmd-b")?;
    anyhow::ensure!(
        !ui.state().sidebar_open,
        "⌘B from {from} did not collapse the sidebar"
    );
    ui.press("cmd-b")?;
    anyhow::ensure!(
        ui.state().sidebar_open,
        "⌘B from {from} did not reopen the sidebar"
    );
    ui.click("collapse-sidebar")?;
    anyhow::ensure!(
        !ui.state().sidebar_open,
        "the chevron from {from} did not collapse it"
    );
    ui.click("expand-sidebar")?;
    anyhow::ensure!(
        ui.state().sidebar_open,
        "the chevron from {from} did not expand it"
    );
    ui.press("cmd-b")?;
    ui.click("expand-sidebar")?;
    anyhow::ensure!(
        ui.state().sidebar_open,
        "⌘B then the chevron from {from} left it closed"
    );
    ui.click("collapse-sidebar")?;
    ui.press("cmd-b")?;
    anyhow::ensure!(
        ui.state().sidebar_open,
        "the chevron then ⌘B from {from} left it closed"
    );
    Ok(())
}

fn all_namespaces(ui: &mut Ui, picker: &'static str) -> anyhow::Result<()> {
    ui.click(picker)?;
    ui.click(selector(format!("{picker}-option-0")))?;
    ui.wait_until(PAGE_TIMEOUT, ready_here)
}

#[test]
fn live_sidebar_toggles_with_command_b_and_the_chevron_from_every_focus() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, CLUSTERS[0].0)?;
    visit(&mut ui, Page::Deployments)?;
    all_namespaces(&mut ui, "resource-namespace")?;
    ui.click("resources-Deployment-row-0")?;
    sidebar_round_trip(&mut ui, "a content row")?;
    anyhow::ensure!(
        ui.bounds("resource-inspector-pane").is_some(),
        "the inspector closed"
    );
    ui.click("close-resource")?;
    ui.press("cmd-e down")?;
    sidebar_round_trip(&mut ui, "the inspector")?;
    visit(&mut ui, Page::Pods)?;
    all_namespaces(&mut ui, "pod-namespace")?;
    ui.click("pods-row-0")?;
    ui.click("dock-tab-0")?;
    sidebar_round_trip(&mut ui, "the dock")?;
    ui.press("cmd-k escape")?;
    sidebar_round_trip(&mut ui, "the palette just closed")?;
    ui.press("cmd-shift-e")?;
    sidebar_round_trip(&mut ui, "the sidebar")?;
    ui.press("/")?;
    sidebar_round_trip(&mut ui, "the page filter")?;
    ui.press("escape cmd-,")?;
    anyhow::ensure!(
        ui.state().location().page == Page::Settings,
        "⌘, did not open settings"
    );
    sidebar_round_trip(&mut ui, "settings")?;
    check_invariants(&mut ui, &before, "toggling the sidebar")
}

#[derive(Clone, Copy, Debug)]
enum Focus {
    Content,
    Inspector,
    Dock,
    Palette,
    Settings,
}

fn focus(ui: &mut Ui, focus: Focus) -> anyhow::Result<()> {
    match focus {
        Focus::Content => {
            visit(ui, Page::Deployments)?;
            ui.click("resources-Deployment-row-0")?;
            ui.click("close-resource")?;
            ui.press("cmd-e")
        }
        Focus::Inspector => {
            visit(ui, Page::Deployments)?;
            ui.click("resources-Deployment-row-0")
        }
        Focus::Dock => {
            visit(ui, Page::Pods)?;
            ui.click("pods-row-0")?;
            ui.click("dock-tab-0")
        }
        Focus::Palette => ui.press("cmd-k escape"),
        Focus::Settings => ui.press("cmd-,"),
    }
}

const FOCUSES: [Focus; 5] = [
    Focus::Content,
    Focus::Inspector,
    Focus::Dock,
    Focus::Palette,
    Focus::Settings,
];

fn shortcuts_from(ui: &mut Ui, from: Focus) -> anyhow::Result<()> {
    let tabs = tab_count(ui);
    ui.press("cmd-t")?;
    anyhow::ensure!(tab_count(ui) == tabs + 1, "⌘T from {from:?}");
    ui.press("cmd-w")?;
    anyhow::ensure!(tab_count(ui) == tabs, "⌘W from {from:?}");
    for (keys, open) in [
        ("cmd-k", "the palette"),
        ("cmd-p", "quick open"),
        ("cmd-shift-p", "the command palette"),
    ] {
        ui.press(keys)?;
        let palette = ui.read(flock_ui::Shell::palette_open);
        anyhow::ensure!(palette, "{keys} from {from:?} did not open {open}");
        ui.press("escape")?;
    }
    ui.press("cmd-shift-c")?;
    anyhow::ensure!(ui.read(flock_ui::Shell::switcher_open), "⌘⇧C from {from:?}");
    ui.press("escape")?;
    let start = ui.state().location().clone();
    if start.cluster.is_some() {
        ui.press("cmd-0")?;
        anyhow::ensure!(ui.state().location().cluster.is_none(), "⌘0 from {from:?}");
        ui.press("cmd-[")?;
        anyhow::ensure!(
            ui.state().location() == &start,
            "⌘[ from {from:?} did not return to {start:?}"
        );
        ui.press("cmd-]")?;
        anyhow::ensure!(ui.state().location().cluster.is_none(), "⌘] from {from:?}");
        ui.press("cmd-[")?;
    }
    ui.press("cmd-b cmd-b")?;
    anyhow::ensure!(ui.state().sidebar_open, "⌘B twice from {from:?}");
    ui.press("cmd-,")?;
    anyhow::ensure!(
        ui.state().location().page == Page::Settings,
        "⌘, from {from:?}"
    );
    if start.page != Page::Settings {
        ui.press("cmd-[")?;
    }
    Ok(())
}

#[test]
fn live_app_shortcuts_work_from_every_focus() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    let cluster = ClusterId::new(CLUSTERS[0].0);
    switch_to(&mut ui, cluster.as_str())?;
    visit(&mut ui, Page::Deployments)?;
    all_namespaces(&mut ui, "resource-namespace")?;
    visit(&mut ui, Page::Pods)?;
    all_namespaces(&mut ui, "pod-namespace")?;
    for from in FOCUSES {
        focus(&mut ui, from)?;
        shortcuts_from(&mut ui, from).with_context(|| format!("from {from:?}"))?;
        check_invariants(&mut ui, &before, &format!("shortcuts from {from:?}"))?;
        if let Focus::Settings = from {
            switch_to(&mut ui, cluster.as_str())?;
        }
    }
    Ok(())
}

struct ListPage {
    page: Page,
    table: &'static str,
    picker: Option<&'static str>,
    inspector: Option<&'static str>,
    menu: bool,
}

const fn list(
    page: Page,
    table: &'static str,
    picker: Option<&'static str>,
    inspector: Option<&'static str>,
    menu: bool,
) -> ListPage {
    ListPage {
        page,
        table,
        picker,
        inspector,
        menu,
    }
}

const RESOURCE: Option<&str> = Some("resource-namespace");
const RESOURCE_PANE: Option<&str> = Some("resource-inspector-pane");

const LISTS: [ListPage; 18] = [
    list(
        Page::Nodes,
        "nodes",
        None,
        Some("node-inspector-pane"),
        true,
    ),
    list(Page::Namespaces, "namespaces", None, None, false),
    list(
        Page::Events,
        "events",
        Some("events-namespace"),
        Some("event-inspector-pane"),
        false,
    ),
    list(Page::Pods, "pods", Some("pod-namespace"), None, true),
    list(
        Page::Deployments,
        "resources-Deployment",
        RESOURCE,
        None,
        true,
    ),
    list(
        Page::StatefulSets,
        "resources-StatefulSet",
        RESOURCE,
        None,
        true,
    ),
    list(
        Page::DaemonSets,
        "resources-DaemonSet",
        RESOURCE,
        None,
        true,
    ),
    list(Page::Jobs, "resources-Job", RESOURCE, RESOURCE_PANE, true),
    list(
        Page::CronJobs,
        "resources-CronJob",
        RESOURCE,
        RESOURCE_PANE,
        true,
    ),
    list(Page::Kustomizations, "flux-list", None, None, true),
    list(Page::HelmReleases, "flux-list", None, None, true),
    list(Page::Sources, "flux-list", None, None, true),
    list(
        Page::Services,
        "resources-Service",
        RESOURCE,
        RESOURCE_PANE,
        true,
    ),
    list(
        Page::Ingresses,
        "resources-Ingress",
        RESOURCE,
        RESOURCE_PANE,
        true,
    ),
    list(
        Page::ConfigMaps,
        "resources-ConfigMap",
        RESOURCE,
        RESOURCE_PANE,
        true,
    ),
    list(
        Page::Secrets,
        "resources-Secret",
        RESOURCE,
        RESOURCE_PANE,
        true,
    ),
    list(
        Page::Pvcs,
        "resources-PersistentVolumeClaim",
        RESOURCE,
        RESOURCE_PANE,
        true,
    ),
    list(Page::Pods, "pods", Some("pod-namespace"), None, true),
];

const EMPTY_STATES: [&str; 4] = [
    "empty-state",
    "empty-state-cluster",
    "empty-state-namespace",
    "empty-state-filter",
];

fn empty_shown(ui: &mut Ui) -> bool {
    EMPTY_STATES
        .iter()
        .any(|selector| ui.bounds(selector).is_some())
}

fn exercise_list(ui: &mut Ui, list: &ListPage) -> anyhow::Result<Option<Page>> {
    let page = list.page;
    visit(ui, page)?;
    let filtered = filter(ui, "zz-no-such-object");
    if filtered.is_err() && empty_shown(ui) {
        return Ok(Some(page));
    }
    filtered.with_context(|| format!("{page:?}"))?;
    ui.press("escape")?;
    anyhow::ensure!(
        filter_words(ui).is_empty(),
        "{page:?}: escape kept the filter"
    );
    if let Some(picker) = list.picker {
        if ui.bounds(picker).is_none() && empty_shown(ui) {
            return Ok(Some(page));
        }
        ui.click(picker)?;
        let option = selector(format!("{picker}-option-0"));
        anyhow::ensure!(
            ui.bounds(option).is_some(),
            "{page:?}: the namespace picker did not open"
        );
        ui.click(option)?;
        ui.wait_until(PAGE_TIMEOUT, ready_here)?;
    }
    let row = selector(format!("{}-row-0", list.table));
    if ui.bounds(row).is_none() {
        anyhow::ensure!(
            empty_shown(ui) || ui.bounds(list.table).is_some(),
            "{page:?} in all namespaces shows neither rows nor an empty state"
        );
        return Ok(None);
    }
    if let Some(inspector) = list.inspector {
        ui.click(row)?;
        anyhow::ensure!(
            ui.bounds(inspector).is_some(),
            "{page:?}: no inspector after a row click"
        );
        ui.press("escape")?;
        anyhow::ensure!(
            ui.bounds(inspector).is_none(),
            "{page:?}: escape kept the inspector"
        );
    }
    if list.menu {
        ui.right_click(row)?;
        let item = selector(format!("{}-row-menu-item-0", list.table));
        anyhow::ensure!(
            ui.bounds(item).is_some(),
            "{page:?}: right click opened no row menu"
        );
        ui.press("escape")?;
        anyhow::ensure!(
            ui.bounds(item).is_none(),
            "{page:?}: escape kept the row menu"
        );
        ui.hover(row)?;
        let more = selector(format!("{}-row-0-more", list.table));
        anyhow::ensure!(
            ui.bounds(more).is_some(),
            "{page:?}: hovering a row shows no ⋯"
        );
    }
    anyhow::ensure!(
        ui.state().location().page == page,
        "{page:?}: the page changed"
    );
    Ok(None)
}

fn exercise_lists(make: &BackendFactory) -> anyhow::Result<Vec<String>> {
    let (mut ui, before) = opened(make, Screen::Flock)?;
    let mut hidden = Vec::new();
    for (name, _) in CLUSTERS {
        switch_to(&mut ui, name)?;
        for list in &LISTS {
            let controls_hidden =
                exercise_list(&mut ui, list).with_context(|| format!("on {name}"))?;
            hidden.extend(controls_hidden.map(|page| format!("{name}/{page:?}")));
            check_invariants(
                &mut ui,
                &before,
                &format!("{name} {:?} controls", list.page),
            )?;
        }
    }
    Ok(hidden)
}

#[test]
fn live_every_inspector_row_menu_filter_and_namespace_picker_opens() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let hidden = exercise_lists(&make)?;
    anyhow::ensure!(
        hidden.is_empty(),
        "empty lists hid the filter bar or namespace picker on {hidden:?}"
    );
    Ok(())
}

const SUSPEND_ITEM: &str = "flux-actions-item-1";
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
const SUSPENDED_RELEASE: (&str, &str) = ("search", "search-indexer");
const RELEASE_MANIFEST: &str = "release.json";

fn flux_object<'a>(
    state: &'a AppState,
    cluster: &str,
    name: &str,
) -> Option<&'a flock_core::FluxObject> {
    state
        .snapshot
        .flux
        .iter()
        .find(|object| object.key.cluster.as_str() == cluster && object.key.name == name)
}

fn suspended(state: &AppState, cluster: &str, name: &str) -> Option<bool> {
    flux_object(state, cluster, name).map(|object| object.phase.is_suspended())
}

fn error_toasts(ui: &Ui) -> usize {
    ui.read_app(flock_ui::kit::toast_statuses)
        .into_iter()
        .filter(|status| *status == Status::Err)
        .count()
}

struct PristineRelease {
    context: &'static str,
    manifest: PathBuf,
    _dir: tempfile::TempDir,
}

impl PristineRelease {
    fn save(context: &'static str) -> anyhow::Result<Self> {
        let (namespace, name) = SUSPENDED_RELEASE;
        let output = std::process::Command::new("kubectl")
            .args([
                "--context",
                context,
                "-n",
                namespace,
                "get",
                "helmrelease",
                name,
            ])
            .args(["-o", "json"])
            .output()
            .context("running kubectl")?;
        anyhow::ensure!(output.status.success(), "reading {namespace}/{name}");
        let live: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let pristine = serde_json::json!({
            "apiVersion": live["apiVersion"],
            "kind": live["kind"],
            "metadata": { "name": name, "namespace": namespace },
            "spec": live["spec"],
        });
        let dir = tempfile::tempdir()?;
        let manifest = dir.path().join(RELEASE_MANIFEST);
        std::fs::write(&manifest, serde_json::to_vec(&pristine)?)?;
        Ok(Self {
            context,
            manifest,
            _dir: dir,
        })
    }

    fn restore(&self) -> anyhow::Result<()> {
        let (namespace, name) = SUSPENDED_RELEASE;
        let helm = std::process::Command::new("helm")
            .args([
                "--kube-context",
                self.context,
                "-n",
                namespace,
                "uninstall",
                name,
            ])
            .args(["--ignore-not-found"])
            .output()
            .context("running helm")?;
        anyhow::ensure!(
            helm.status.success(),
            "helm uninstall {name}: {}",
            String::from_utf8_lossy(&helm.stderr)
        );
        kubectl(
            self.context,
            &[
                "-n",
                namespace,
                "delete",
                "helmrelease",
                name,
                "--ignore-not-found",
            ],
        )?;
        let manifest = self.manifest.to_string_lossy();
        kubectl(self.context, &["create", "-f", &manifest])
    }
}

impl Drop for PristineRelease {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!(
                "could not restore the suspended release on {}: {error:#}",
                self.context
            );
        }
    }
}

fn filter_words(ui: &Ui) -> Vec<String> {
    ui.read(|shell, cx| shell.filter_bar(cx).map(|bar| bar.read(cx).words()))
        .unwrap_or_default()
}

fn filter(ui: &mut Ui, text: &str) -> anyhow::Result<()> {
    ui.press("/")?;
    if !filter_words(ui).is_empty() {
        ui.press("escape")?;
    }
    ui.type_text(text)?;
    let words = filter_words(ui);
    anyhow::ensure!(
        words == [text],
        "the filter holds {words:?} instead of {text}"
    );
    Ok(())
}

fn open_flux_object(ui: &mut Ui, page: Page, name: &str) -> anyhow::Result<()> {
    visit(ui, page)?;
    filter(ui, name)?;
    ui.double_click("flux-list-row-0")
        .with_context(|| format!("no {page:?} row matches {name}"))?;
    let opened = ui.state().location().flux_object.clone();
    anyhow::ensure!(
        opened.as_ref().is_some_and(|key| key.name == name),
        "opening {name} landed on {opened:?}"
    );
    ui.wait_until(PAGE_TIMEOUT, |state| {
        state
            .location()
            .flux_object
            .as_ref()
            .is_some_and(|key| flux_object(state, key.cluster.as_str(), &key.name).is_some())
    })
}

#[test]
fn live_flux_pages_reconcile_and_resume_then_suspend_the_suspended_release() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (cluster, context) = CLUSTERS[0];
    let _restore = PristineRelease::save(context)?;
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, cluster)?;
    for (page, name) in [
        (Page::Kustomizations, "apps"),
        (Page::Kustomizations, "broken-kustomization"),
        (Page::HelmReleases, "podinfo"),
        (Page::Sources, "podinfo"),
    ] {
        open_flux_object(&mut ui, page, name)?;
        anyhow::ensure!(
            ui.bounds("error-state").is_none(),
            "{name} shows an error state"
        );
        ui.click("flux-reconcile")?;
        ui.pause(Duration::from_secs(2))?;
        anyhow::ensure!(
            error_toasts(&ui) == 0,
            "reconciling {name} raised {:?}",
            toasts(&ui)
        );
        ui.click("flux-actions")?;
        anyhow::ensure!(
            ui.bounds("flux-actions-item-0").is_some(),
            "{name} has no Actions menu"
        );
        ui.press("escape")?;
    }
    let (_, release) = SUSPENDED_RELEASE;
    open_flux_object(&mut ui, Page::HelmReleases, release)?;
    anyhow::ensure!(
        suspended(&ui.state(), cluster, release) == Some(true),
        "{release} starts suspended"
    );
    ui.click("flux-actions")?;
    ui.click(SUSPEND_ITEM)?;
    ui.wait_until(ACTION_TIMEOUT, |state| {
        suspended(state, cluster, release) == Some(false)
    })
    .context("Resume did not resume the release")?;
    ui.click("flux-actions")?;
    ui.click(SUSPEND_ITEM)?;
    ui.wait_until(ACTION_TIMEOUT, |state| {
        suspended(state, cluster, release) == Some(true)
    })
    .context("Suspend did not suspend the release again")?;
    anyhow::ensure!(
        error_toasts(&ui) == 0,
        "Flux actions raised {:?}",
        toasts(&ui)
    );
    let counts = missing_counts(&before, &ui.state());
    anyhow::ensure!(counts.is_empty(), "Flux actions lost counts {counts:?}");
    Ok(())
}

#[test]
fn live_pod_terminal_opens_a_shell_only_on_request_and_exits() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, CLUSTERS[0].0)?;
    visit(&mut ui, Page::Pods)?;
    all_namespaces(&mut ui, "pod-namespace")?;
    filter(&mut ui, "podinfo")?;
    ui.click("pods-row-0")?;
    ui.click("pod-shell")?;
    anyhow::ensure!(
        ui.bounds("pod-open-shell").is_some(),
        "the terminal tab shows no Open shell"
    );
    anyhow::ensure!(
        ui.bounds("pod-close-shell").is_none(),
        "the terminal started a shell by itself"
    );
    ui.click("pod-open-shell")?;
    ui.wait_until(PAGE_TIMEOUT, |_| true)?;
    anyhow::ensure!(
        ui.bounds("pod-close-shell").is_some(),
        "Open shell did not start a session"
    );
    ui.type_text("exit")?;
    ui.press("enter")?;
    let start = std::time::Instant::now();
    while ui.bounds("pod-open-shell").is_none() {
        anyhow::ensure!(start.elapsed() < PAGE_TIMEOUT, "exit did not end the shell");
        ui.pause(Duration::from_millis(500))?;
    }
    check_invariants(&mut ui, &before, "using the terminal")
}

const LIVE_WAIT: Duration = Duration::from_secs(60);
const LIVE_CONFIG_MAP: &str = "flock-qa-live";

fn kubectl(context: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new("kubectl")
        .args(["--context", context])
        .args(args)
        .output()
        .context("running kubectl")?;
    anyhow::ensure!(
        output.status.success(),
        "kubectl {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn config_map_listed(state: &AppState, cluster: &str) -> bool {
    state
        .snapshot
        .details
        .get(&ClusterId::new(cluster))
        .and_then(|detail| detail.resources.get(&flock_core::ResourceKind::ConfigMap))
        .is_some_and(|rows| rows.iter().any(|row| row.name == LIVE_CONFIG_MAP))
}

struct DeleteConfigMap {
    context: &'static str,
}

impl Drop for DeleteConfigMap {
    fn drop(&mut self) {
        let deleted = kubectl(
            self.context,
            &[
                "-n",
                "default",
                "delete",
                "configmap",
                LIVE_CONFIG_MAP,
                "--ignore-not-found",
            ],
        );
        if let Err(error) = deleted {
            eprintln!("could not delete {LIVE_CONFIG_MAP}: {error}");
        }
    }
}

#[test]
fn live_a_page_left_open_for_a_minute_follows_the_cluster() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (cluster, context) = CLUSTERS[1];
    let _cleanup = DeleteConfigMap { context };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, cluster)?;
    visit(&mut ui, Page::ConfigMaps)?;
    let start = std::time::Instant::now();
    while start.elapsed() < LIVE_WAIT {
        ui.pause(Duration::from_secs(5))?;
        anyhow::ensure!(
            ui.bounds("loading-skeleton").is_none(),
            "the page fell back to a skeleton"
        );
        anyhow::ensure!(
            ui.bounds("error-state").is_none(),
            "the page fell into an error state"
        );
        check_invariants(&mut ui, &before, "waiting on ConfigMaps")?;
    }
    kubectl(
        context,
        &["-n", "default", "create", "configmap", LIVE_CONFIG_MAP],
    )?;
    ui.wait_until(PAGE_TIMEOUT, |state| config_map_listed(state, cluster))
        .context("a new ConfigMap never appeared")?;
    let row = "resources-ConfigMap-row-0";
    anyhow::ensure!(
        ui.bounds(row).is_some(),
        "the new ConfigMap has no row in default"
    );
    kubectl(
        context,
        &["-n", "default", "delete", "configmap", LIVE_CONFIG_MAP],
    )?;
    ui.wait_until(PAGE_TIMEOUT, |state| !config_map_listed(state, cluster))
        .context("a deleted ConfigMap stayed listed")?;
    check_invariants(&mut ui, &before, "creating and deleting a ConfigMap")
}

#[test]
fn live_the_switcher_moves_between_clusters_and_back_to_all() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    for (name, _) in CLUSTERS {
        switch_to(&mut ui, name)?;
        visit(&mut ui, Page::Deployments)?;
    }
    ui.click("cluster-switcher")?;
    ui.click("switcher-row-1")?;
    anyhow::ensure!(
        ui.state()
            .location()
            .cluster
            .as_ref()
            .map(ClusterId::as_str)
            == Some(CLUSTERS[0].0),
        "the first cluster row went to {:?}",
        ui.state().location()
    );
    anyhow::ensure!(
        ui.state().location().page == Page::Deployments,
        "switching kept the page"
    );
    ui.press("cmd-0")?;
    anyhow::ensure!(
        ui.state().location().cluster.is_none(),
        "⌘0 did not show all clusters"
    );
    ui.press("cmd-[")?;
    anyhow::ensure!(
        ui.state().location().cluster.is_some(),
        "⌘[ did not return to the cluster"
    );
    check_invariants(&mut ui, &before, "switching clusters")
}

#[cfg(feature = "loco")]
const LOCO_CLUSTER: &str = "flock-loco";
#[cfg(feature = "loco")]
const LOCO_APPLICATIONS: usize = 60;
#[cfg(feature = "loco")]
const LOCO_BUILDS: usize = 8;
#[cfg(feature = "loco")]
const LOCO_ROUTES: usize = 29;
#[cfg(feature = "loco")]
const LOCO_PAGES: [(Page, &str); 4] = [
    (Page::LocoPlatform, "loco-components-row-0"),
    (Page::LocoApplications, "loco-apps-row-1"),
    (Page::LocoBuilds, "loco-builds-row-0"),
    (Page::LocoRoutes, "loco-routes-row-0"),
];

#[cfg(feature = "loco")]
fn loco_loaded(state: &AppState, page: Page) -> bool {
    let Some(loco) = state.snapshot.loco.get(&ClusterId::new(LOCO_CLUSTER)) else {
        return false;
    };
    let counted = state
        .snapshot
        .cluster(&ClusterId::new(LOCO_CLUSTER))
        .and_then(|cluster| cluster.nav.get(&page))
        .is_some_and(|counts| match page {
            Page::LocoPlatform => counts.warn > 0,
            Page::LocoBuilds => counts.err > 0,
            _ => counts.total.is_some() && counts.err + counts.warn > 0,
        });
    let rows = match page {
        Page::LocoPlatform => !loco.platform.components.is_empty(),
        Page::LocoApplications => loco.applications.total >= LOCO_APPLICATIONS,
        Page::LocoBuilds => loco.builds.builds.len() >= LOCO_BUILDS,
        Page::LocoRoutes => loco.routes.total >= LOCO_ROUTES,
        _ => false,
    };
    ready_here(state) && counted && rows
}

#[cfg(feature = "loco")]
fn loco_summary(state: &AppState, page: Page) -> String {
    let id = ClusterId::new(LOCO_CLUSTER);
    let counts = state
        .snapshot
        .cluster(&id)
        .and_then(|cluster| cluster.nav.get(&page).copied());
    let loco = state.snapshot.loco.get(&id).map(|loco| {
        (
            loco.platform.components.len(),
            loco.applications.total,
            loco.builds.builds.len(),
            loco.routes.total,
        )
    });
    format!("counts {counts:?}, components/applications/builds/routes {loco:?}")
}

#[cfg(feature = "loco")]
#[test]
fn live_every_loco_page_loads_real_loco_with_counts_and_no_toasts() -> anyhow::Result<()> {
    let Some(make) = kind()? else {
        return Ok(());
    };
    let (mut ui, before) = opened(&make, Screen::Flock)?;
    switch_to(&mut ui, CLUSTERS[0].0)?;
    anyhow::ensure!(
        ui.bounds(nav(Page::LocoPlatform)).is_none(),
        "{} shows the Loco section without Loco",
        CLUSTERS[0].0
    );
    switch_to(&mut ui, LOCO_CLUSTER)?;
    for round in ["first visit", "return"] {
        for (page, row) in LOCO_PAGES {
            let step = format!("{round} to {LOCO_CLUSTER} {page:?}");
            visit(&mut ui, page)?;
            ui.wait_until(PAGE_TIMEOUT, |state| loco_loaded(state, page))
                .with_context(|| {
                    format!(
                        "{step} never loaded its Loco rows and counts: {}",
                        loco_summary(&ui.state(), page)
                    )
                })?;
            anyhow::ensure!(
                ui.bounds("loading-skeleton").is_none(),
                "{step} is stuck loading"
            );
            anyhow::ensure!(
                ui.bounds("error-state").is_none(),
                "{step} shows an error state"
            );
            ui.click(row)
                .with_context(|| format!("{step} has no {row}"))?;
            ui.pause(Duration::from_secs(1))?;
            anyhow::ensure!(
                ui.bounds("error-state").is_none(),
                "{step}: selecting {row} shows an error state"
            );
            if ui.state().location().page != page {
                ui.press("cmd-[")?;
                ui.wait_until(PAGE_TIMEOUT, ready_here)?;
            }
            anyhow::ensure!(
                ui.state().location().page == page,
                "{step}: going back after {row} landed on {:?}",
                ui.state().location().page
            );
            check_invariants(&mut ui, &before, &step)?;
        }
    }
    Ok(())
}
