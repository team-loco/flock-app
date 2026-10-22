use anyhow::Context as _;
use flock::Screen;
use flock_core::resource::page_for_kind;
use flock_core::resources::{EVENT_LIST, POD_LIST};
use flock_core::{ClusterId, ClusterSummary, Command, Page};
use flock_shots::Ui;
use flock_ui::Shell;
use flock_ui::theme::{
    FONT_MONO, FONT_SANS, FONT_STATUS, ICON_BUTTON, INSPECTOR_IN, SIDEBAR, SIDEBAR_CLOSE,
    SIDEBAR_MAX, SIDEBAR_MIN, SPACE_3, SPACE_5, SPLITTER, TAB_ENTER, TAB_LEAVE, TEXT_MONO, TEXT_UI,
    TITLE_ROW, TRAFFIC_LIGHTS_INSET,
};
use gpui::{Pixels, SharedString, TextRun, font, rgb};

const EXPECTED_WIDTH: u32 = 2880;
const EXPECTED_HEIGHT: u32 = 1800;
const TAB_STRIP_REWIND: f32 = 1000.;
const CENTRE_TOLERANCE: f32 = 1.;

#[test]
fn command_k_opens_the_palette_and_enter_runs_the_top_hit() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    assert_eq!(ui.state().location().page, Page::Overview);
    ui.press("cmd-k")?;
    ui.type_text("gitops")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::GitOps);
    Ok(())
}

fn width(ui: &mut Ui, selector: &'static str) -> Option<Pixels> {
    ui.bounds(selector).map(|bounds| bounds.size.width)
}

fn left(ui: &mut Ui, selector: &'static str) -> Option<Pixels> {
    ui.bounds(selector).map(|bounds| bounds.origin.x)
}

#[test]
fn command_b_hides_the_sidebar_and_insets_the_tab_strip() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let before = ui.state();
    assert!(before.sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(SIDEBAR));
    assert!(ui.bounds("window-controls").is_none());
    assert_eq!(left(&mut ui, "tab-0"), Some(SIDEBAR));
    ui.press("cmd-b")?;
    assert!(!ui.state().sidebar_open);
    assert_eq!(ui.state().location().cluster, before.location().cluster);
    assert_eq!(width(&mut ui, "sidebar"), Some(Pixels::ZERO));
    let controls = TRAFFIC_LIGHTS_INSET + ICON_BUTTON + SPACE_3;
    assert_eq!(width(&mut ui, "window-controls"), Some(controls));
    assert_eq!(left(&mut ui, "tab-0"), Some(controls));
    ui.press("cmd-b")?;
    assert!(ui.state().sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(SIDEBAR));
    assert_eq!(width(&mut ui, "window-controls"), Some(Pixels::ZERO));
    assert_eq!(left(&mut ui, "tab-0"), Some(SIDEBAR));
    Ok(())
}

#[test]
fn the_expand_chevron_sits_after_the_traffic_lights_in_the_title_row() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-b")?;
    let chevron = ui
        .bounds("expand-sidebar")
        .ok_or_else(|| anyhow::anyhow!("no expand chevron"))?;
    assert_eq!(chevron.origin.x, TRAFFIC_LIGHTS_INSET);
    assert_eq!(chevron.center().y, TITLE_ROW / 2.);
    ui.click("expand-sidebar")?;
    assert!(ui.state().sidebar_open);
    Ok(())
}

fn saved_sidebar(ui: &Ui) -> (Option<u16>, bool) {
    ui.read_app(|cx| {
        let settings = flock_ui::settings::current(cx);
        (settings.sidebar_width, settings.sidebar_closed)
    })
}

fn drag_edge(ui: &mut Ui, by: Pixels) -> anyhow::Result<()> {
    ui.drag("sidebar-resize", gpui::point(by, Pixels::ZERO))
}

#[test]
fn the_sidebar_has_no_collapse_arrow_and_an_invisible_edge_handle() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds("collapse-sidebar").is_none());
    let edge = ui
        .bounds("sidebar-resize")
        .ok_or_else(|| anyhow::anyhow!("no sidebar edge handle"))?;
    assert_eq!(edge.size.width, SPLITTER);
    assert_eq!(edge.origin.x + edge.size.width, SIDEBAR);
    assert!(ui.bounds("sidebar-reopen").is_none());
    Ok(())
}

#[test]
fn dragging_the_sidebar_edge_resizes_it_within_its_clamps() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let wider = SIDEBAR + gpui::px(60.);
    drag_edge(&mut ui, wider - SIDEBAR)?;
    assert_eq!(width(&mut ui, "sidebar"), Some(wider));
    assert_eq!(left(&mut ui, "tab-0"), Some(wider));
    assert_eq!(saved_sidebar(&ui), (Some(260), false));
    drag_edge(&mut ui, gpui::px(1000.))?;
    assert_eq!(width(&mut ui, "sidebar"), Some(SIDEBAR_MAX));
    let between = SIDEBAR_CLOSE + (SIDEBAR_MIN - SIDEBAR_CLOSE) / 2.;
    drag_edge(&mut ui, between - SIDEBAR_MAX)?;
    assert!(ui.state().sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(SIDEBAR_MIN));
    assert_eq!(left(&mut ui, "tab-0"), Some(SIDEBAR_MIN));
    let scope = ui
        .bounds("sidebar-scope")
        .ok_or_else(|| anyhow::anyhow!("no sidebar scope"))?;
    assert!(scope.origin.x + scope.size.width <= SIDEBAR_MIN);
    Ok(())
}

#[test]
fn dragging_the_sidebar_past_the_threshold_closes_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    drag_edge(&mut ui, SIDEBAR_CLOSE - SIDEBAR - gpui::px(1.))?;
    assert!(!ui.state().sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(Pixels::ZERO));
    assert!(ui.bounds("sidebar-scope").is_none());
    assert_eq!(saved_sidebar(&ui), (None, true));
    assert!(ui.bounds("sidebar-reopen").is_some());
    Ok(())
}

#[test]
fn dragging_from_the_left_edge_reopens_the_sidebar() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-b")?;
    let handle = ui
        .bounds("sidebar-reopen")
        .ok_or_else(|| anyhow::anyhow!("no reopen handle"))?;
    assert_eq!(handle.origin.x, Pixels::ZERO);
    assert_eq!(handle.origin.y, TITLE_ROW);
    assert_eq!(handle.size.width, SPLITTER);
    ui.drag(
        "sidebar-reopen",
        gpui::point(SIDEBAR_CLOSE / 2., Pixels::ZERO),
    )?;
    assert!(!ui.state().sidebar_open);
    let opened = gpui::px(240.);
    ui.drag("sidebar-reopen", gpui::point(opened, Pixels::ZERO))?;
    assert!(ui.state().sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(opened));
    assert_eq!(left(&mut ui, "tab-0"), Some(opened));
    assert_eq!(saved_sidebar(&ui), (Some(240), false));
    assert!(ui.bounds("sidebar-reopen").is_none());
    Ok(())
}

#[test]
fn the_reopen_handle_lets_clicks_through_to_the_content_under_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    ui.press("cmd-b")?;
    let row = ui
        .bounds("resources-Deployment-row-0")
        .ok_or_else(|| anyhow::anyhow!("no deployment row"))?;
    assert_eq!(row.origin.x, Pixels::ZERO);
    assert_eq!(ui.state().location().inspecting, None);
    ui.click_at(gpui::point(SPLITTER / 2., row.center().y), 1)?;
    assert!(ui.state().location().inspecting.is_some());
    assert!(!ui.state().sidebar_open);
    Ok(())
}

#[test]
fn command_b_restores_the_last_dragged_width() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let dragged = gpui::px(280.);
    drag_edge(&mut ui, dragged - SIDEBAR)?;
    ui.press("cmd-b")?;
    assert!(!ui.state().sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(Pixels::ZERO));
    assert_eq!(saved_sidebar(&ui), (Some(280), true));
    ui.press("cmd-b")?;
    assert!(ui.state().sidebar_open);
    assert_eq!(width(&mut ui, "sidebar"), Some(dragged));
    assert_eq!(left(&mut ui, "tab-0"), Some(dragged));
    assert_eq!(saved_sidebar(&ui), (Some(280), false));
    drag_edge(&mut ui, -dragged)?;
    assert!(!ui.state().sidebar_open);
    ui.press("cmd-b")?;
    assert_eq!(width(&mut ui, "sidebar"), Some(dragged));
    Ok(())
}

fn settings_selection(ui: &Ui) -> Option<ClusterId> {
    ui.read(Shell::cluster_settings_selection)
}

fn cluster_at(ui: &Ui, ix: usize) -> anyhow::Result<ClusterSummary> {
    ui.state()
        .snapshot
        .clusters
        .get(ix)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("the demo has no cluster {ix}"))
}

#[test]
fn clicking_the_switcher_opens_it_and_a_row_selects_that_cluster() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    let second = cluster_at(&ui, 1)?;
    let connected = ui.state().snapshot.connected();
    assert!(!ui.read(Shell::switcher_open));
    ui.click("cluster-switcher")?;
    assert!(ui.read(Shell::switcher_open));
    ui.click("switcher-row-2")?;
    let state = ui.state();
    assert_eq!(state.location().cluster, Some(second.id));
    assert_eq!(state.location().page, Page::ClusterOverview);
    assert!(!ui.read(Shell::switcher_open));
    assert_eq!(state.snapshot.connected(), connected);
    Ok(())
}

#[test]
fn typing_in_the_switcher_filters_and_enter_selects() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    let staging = ClusterId::new("staging");
    ui.press("cmd-shift-c")?;
    assert!(ui.read(Shell::switcher_open));
    ui.type_text("stag")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().cluster, Some(staging));
    ui.press("cmd-shift-c")?;
    ui.type_text("all")?;
    ui.press("enter")?;
    let state = ui.state();
    assert_eq!(state.location().cluster, None);
    assert_eq!(state.location().page, Page::Overview);
    Ok(())
}

#[test]
fn arrow_keys_move_from_the_current_cluster() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let first = cluster_at(&ui, 0)?;
    let second = cluster_at(&ui, 1)?;
    assert_eq!(ui.state().location().cluster, Some(first.id));
    ui.press("cmd-shift-c down enter")?;
    assert_eq!(ui.state().location().cluster, Some(second.id));
    ui.press("cmd-shift-c up up enter")?;
    assert_eq!(ui.state().location().cluster, None);
    ui.press("cmd-shift-c escape")?;
    assert!(!ui.read(Shell::switcher_open));
    assert_eq!(ui.state().location().cluster, None);
    Ok(())
}

#[test]
fn cluster_actions_open_from_the_switcher() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-shift-c cmd-.")?;
    assert!(ui.bounds("switcher-menu").is_some());
    assert!(ui.bounds("switcher-row-0").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("switcher-menu").is_none());
    assert!(ui.read(Shell::switcher_open));
    ui.press("down cmd-.")?;
    ui.click("switcher-action-0")?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    assert_eq!(settings_selection(&ui), Some(cluster_at(&ui, 1)?.id));
    assert!(!ui.read(Shell::switcher_open));
    Ok(())
}

#[cfg(feature = "loco")]
#[test]
fn the_loco_section_shows_only_for_clusters_running_loco() -> anyhow::Result<()> {
    let platform = "nav-platform";
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds(platform).is_some());
    ui.press("cmd-shift-c")?;
    ui.type_text("staging")?;
    ui.press("enter")?;
    assert_eq!(
        ui.state().location().cluster,
        Some(ClusterId::new("staging"))
    );
    assert!(ui.bounds(platform).is_none());
    Ok(())
}

#[test]
fn a_suspended_flux_object_offers_resume_as_its_primary_action() -> anyhow::Result<()> {
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::Kustomization, flock::Preset::default(), appearance)?;
        assert!(ui.bounds("flux-resume").is_none());
        ui.click("flux-actions")?;
        ui.click("flux-actions-item-1")?;
        assert!(ui.bounds("flux-reconcile").is_none());
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        ui.save_png(&dir.join(format!("kustomization-suspended-{appearance:?}.png")))?;
        ui.click("flux-resume")?;
        assert!(ui.bounds("flux-resume").is_none());
        assert!(ui.bounds("flux-reconcile").is_some());
    }
    Ok(())
}

#[test]
fn cancelling_a_confirm_dialog_leaves_the_state_unchanged() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    let before = ui.state();
    ui.click("workload-actions")?;
    ui.click("workload-actions-item-6")?;
    assert!(ui.bounds("dialog-cancel").is_some());
    ui.click("dialog-cancel")?;
    assert!(ui.bounds("dialog-cancel").is_none());
    assert_eq!(ui.state(), before);
    Ok(())
}

#[test]
fn clicking_a_cluster_row_selects_that_cluster() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    let second: ClusterId = ui
        .state()
        .snapshot
        .clusters
        .get(1)
        .map(|cluster| cluster.id.clone())
        .ok_or_else(|| anyhow::anyhow!("the demo has fewer than two clusters"))?;
    ui.click("flock-clusters-row-1")?;
    let state = ui.state();
    assert_eq!(state.location().cluster, Some(second));
    assert_eq!(state.location().page, Page::ClusterOverview);
    Ok(())
}

#[test]
fn save_png_renders_the_state_the_steps_reached() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-k")?;
    ui.type_text("nodes")?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("palette.png");
    ui.save_png(&path)?;
    let (width, height) = image::image_dimensions(&path)?;
    assert_eq!((width, height), (EXPECTED_WIDTH, EXPECTED_HEIGHT));
    Ok(())
}

#[test]
fn option_enter_requests_a_window_with_the_cluster_page_without_navigating_the_source()
-> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let before = ui.state();
    ui.press("cmd-k")?;
    ui.type_text("nodes")?;
    ui.press("alt-enter")?;
    assert_eq!(
        ui.take_new_window(),
        Some(vec![
            Command::SelectCluster(before.location().cluster.clone()),
            Command::SelectPage(Page::Nodes)
        ])
    );
    assert_eq!(ui.take_new_window(), None);
    assert_eq!(ui.state(), before);
    Ok(())
}

#[test]
fn command_comma_opens_settings_and_its_controls_change_the_app_settings() -> anyhow::Result<()> {
    use flock_core::app_settings::{PollInterval, ThemeChoice};
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-,")?;
    assert_eq!(ui.state().location().page, Page::Settings);
    let current = |ui: &Ui| ui.read(|_, cx| flock_ui::settings::current(cx).clone());
    assert_eq!(current(&ui).theme, ThemeChoice::System);
    ui.click("settings-theme-2")?;
    assert_eq!(current(&ui).theme, ThemeChoice::Dark);
    ui.click("settings-metrics-0")?;
    assert_eq!(current(&ui).refresh.metrics, PollInterval::Seconds5);
    assert!(current(&ui).notifications.enabled);
    ui.click("settings-notify")?;
    assert!(!current(&ui).notifications.enabled);
    ui.click("settings-clusters")?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    Ok(())
}

#[test]
fn settings_shows_paused_notifications_and_resume_clears_the_pause() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Settings)?;
    let current = |ui: &Ui| ui.read(|_, cx| flock_ui::settings::current(cx).clone());
    assert!(ui.bounds("settings-notify").is_some());
    assert!(ui.bounds("settings-resume").is_none());
    let paused =
        ui.update_app(|cx| flock_ui::settings::update(cx, |s| s.notifications.paused = true));
    assert!(paused.is_ok());
    assert!(ui.bounds("settings-notify").is_none());
    assert!(ui.bounds("settings-resume").is_some());
    ui.click("settings-resume")?;
    assert!(!current(&ui).notifications.paused);
    assert!(current(&ui).notifications.enabled);
    assert!(ui.bounds("settings-notify").is_some());
    Ok(())
}

#[test]
fn menu_bar_toggle_updates_and_saves_the_setting() -> anyhow::Result<()> {
    use std::{cell::RefCell, rc::Rc};
    let mut ui = Ui::open(Screen::Settings)?;
    let saved = Rc::new(RefCell::new(Vec::new()));
    let log = saved.clone();
    ui.update_app(|cx| {
        let current = flock_ui::settings::current(cx).clone();
        flock_ui::settings::install(
            current,
            Rc::new(move |settings| {
                log.borrow_mut().push(settings.show_in_menu_bar);
                Ok(())
            }),
            false,
            cx,
        );
    });
    assert!(ui.read_app(|cx| flock_ui::settings::current(cx).show_in_menu_bar));
    ui.click("settings-menu-bar")?;
    assert!(!ui.read_app(|cx| flock_ui::settings::current(cx).show_in_menu_bar));
    ui.click("settings-menu-bar")?;
    assert!(ui.read_app(|cx| flock_ui::settings::current(cx).show_in_menu_bar));
    assert_eq!(*saved.borrow(), vec![false, true]);
    Ok(())
}

#[test]
fn highlight_swatches_and_hex_fields_recolour_the_app_live() -> anyhow::Result<()> {
    use flock_core::app_settings::{Highlight, Rgb};
    use flock_ui::theme::{ActiveTheme, Palette};
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-,")?;
    let current = |ui: &Ui| ui.read(|_, cx| flock_ui::settings::current(cx).clone());
    let palette = |ui: &Ui| ui.read_app(|cx| *cx.palette());
    let dark = Palette::dark();
    assert_eq!(palette(&ui), dark);
    ui.click("primary-purple")?;
    assert_eq!(current(&ui).primary, Highlight::Purple);
    assert_ne!(palette(&ui).acc, dark.acc);
    assert_eq!(palette(&ui).sel, dark.sel);
    ui.click("secondary-hex")?;
    ui.type_text("#12")?;
    assert_eq!(current(&ui).secondary, Highlight::Blue);
    ui.type_text("ab5g")?;
    assert_eq!(current(&ui).secondary, Highlight::Blue);
    ui.press("backspace backspace")?;
    ui.type_text("56")?;
    let custom = Highlight::Custom(Rgb {
        r: 0x12,
        g: 0xab,
        b: 0x56,
    });
    assert_eq!(current(&ui).secondary, custom);
    assert_ne!(palette(&ui).sel, dark.sel);
    ui.click("secondary-blue")?;
    ui.click("primary-blue")?;
    assert_eq!(palette(&ui), dark);
    Ok(())
}

const DEPLOYMENTS: &str = "resources-Deployment";
const SAMPLE: &str = "prod-us-east";
const DRAG: f32 = 80.;
const ROWS_UNDER_POPOVER: usize = 8;
const AVAILABLE: usize = 4;
const POD_ROWS: usize = 12;
const INSPECTOR_SCROLL: f32 = 400.;
const INSPECTOR_SCROLL_TO_END: f32 = -4000.;
const FIRST_POD: &str = "checkout-api-7f9c6d-x2lqp";
const AVAILABLE_DEFAULT: f32 = 72.;

fn selector(text: String) -> &'static str {
    text.leak()
}

fn handle_x(ui: &mut Ui, column: usize) -> anyhow::Result<f32> {
    ui.bounds(selector(format!("{DEPLOYMENTS}-resize-{column}")))
        .map(|bounds| f32::from(bounds.origin.x))
        .ok_or_else(|| anyhow::anyhow!("column {column} has no resize handle"))
}

fn saved_width(ui: &Ui, scope: &str, column: &str) -> Option<f32> {
    ui.read_app(|cx| flock_ui::prefs::column_widths(scope, DEPLOYMENTS, cx))
        .get(column)
        .copied()
}

#[test]
fn dragging_a_column_border_resizes_it_and_remembers_it_per_cluster() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    let before = handle_x(&mut ui, 0)?;
    ui.drag(
        selector(format!("{DEPLOYMENTS}-resize-0")),
        gpui::point(gpui::px(DRAG), gpui::px(0.)),
    )?;
    let after = handle_x(&mut ui, 0)?;
    assert!((after - before - DRAG).abs() < 1., "{before} → {after}");
    let saved = saved_width(&ui, SAMPLE, "Name");
    assert!(saved.is_some_and(|width| width > DRAG));
    assert_eq!(saved_width(&ui, "staging", "Name"), None);
    ui.press("cmd-shift-c")?;
    ui.type_text("staging")?;
    ui.press("enter")?;
    ui.press("cmd-k")?;
    ui.type_text("deployments")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    assert!((handle_x(&mut ui, 0)? - before).abs() < 1.);
    ui.press("cmd-shift-c")?;
    ui.type_text(SAMPLE)?;
    ui.press("enter")?;
    ui.press("cmd-k")?;
    ui.type_text("deployments")?;
    ui.press("enter")?;
    assert!((handle_x(&mut ui, 0)? - after).abs() < 1.);
    Ok(())
}

#[test]
fn double_clicking_a_column_border_fits_the_column_to_its_content() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    let before = handle_x(&mut ui, AVAILABLE)?;
    ui.double_click(selector(format!("{DEPLOYMENTS}-resize-{AVAILABLE}")))?;
    let fitted = saved_width(&ui, SAMPLE, "Available")
        .ok_or_else(|| anyhow::anyhow!("the fitted width was not saved"))?;
    assert!(fitted >= f32::from(flock_ui::theme::COLUMN_MIN));
    assert!(fitted < AVAILABLE_DEFAULT, "{fitted}");
    let after = handle_x(&mut ui, AVAILABLE)?;
    assert!(after < before, "{before} → {after}");
    Ok(())
}

fn filter_words(ui: &Ui) -> Option<Vec<String>> {
    ui.read(|shell, cx| shell.filter_bar(cx).map(|bar| bar.read(cx).words()))
}

#[test]
fn slash_focuses_the_filter_and_escape_clears_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-1")))
            .is_some()
    );
    let toolbar = ["resource-filter-field", "resource-filter-clear-slot"];
    let idle = toolbar.map(|part| ui.bounds(part));
    assert!(idle.iter().all(Option::is_some), "{idle:?}");
    ui.press("/")?;
    ui.type_text("checkout")?;
    assert_eq!(filter_words(&ui), Some(vec!["checkout".to_owned()]));
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-0")))
            .is_some()
    );
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-1")))
            .is_none()
    );
    assert_eq!(toolbar.map(|part| ui.bounds(part)), idle);
    ui.press("escape")?;
    assert_eq!(filter_words(&ui), Some(Vec::new()));
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-1")))
            .is_some()
    );
    assert_eq!(toolbar.map(|part| ui.bounds(part)), idle);
    Ok(())
}

#[test]
fn pod_toolbar_controls_keep_their_place_when_a_filter_is_applied() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    let toolbar = [
        "pod-filter-field",
        "pod-filter-chips-1",
        "pod-filter-clear-slot",
        "pod-filter-note",
        "pods-logs-bar",
        "pods-row-0",
    ];
    let idle = toolbar.map(|part| ui.bounds(part));
    assert!(idle.iter().all(Option::is_some), "{idle:?}");
    ui.click("pod-filter-chips-1")?;
    assert_eq!(toolbar.map(|part| ui.bounds(part)), idle);
    ui.press("/")?;
    ui.type_text("checkout")?;
    assert_eq!(toolbar.map(|part| ui.bounds(part)), idle);
    ui.click("pod-filter-clear")?;
    assert_eq!(toolbar.map(|part| ui.bounds(part)), idle);
    Ok(())
}

#[test]
fn marking_pods_for_logs_moves_nothing() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    let watched = [
        "pods-logs-bar",
        "pods-clear-selection-slot",
        "pods-row-0",
        "pods-row-2",
    ];
    let idle = watched.map(|part| ui.bounds(part));
    assert!(idle.iter().all(Option::is_some), "{idle:?}");
    ui.click("pods-row-0")?;
    ui.shift_click("pods-row-1")?;
    assert_eq!(watched.map(|part| ui.bounds(part)), idle);
    ui.click("pods-clear-selection")?;
    assert_eq!(watched.map(|part| ui.bounds(part)), idle);
    Ok(())
}

#[test]
fn hovering_an_open_popover_does_not_hover_the_rows_beneath() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    ui.hover(selector(format!("{DEPLOYMENTS}-row-2")))?;
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-2-more")))
            .is_some()
    );
    ui.click("resource-namespace")?;
    for option in 1..=3 {
        ui.hover(selector(format!("resource-namespace-option-{option}")))?;
        for row in 0..ROWS_UNDER_POPOVER {
            let more = selector(format!("{DEPLOYMENTS}-row-{row}-more"));
            assert!(
                ui.bounds(more).is_none(),
                "row {row} is hovered under option {option}"
            );
        }
    }
    Ok(())
}

#[test]
fn right_clicking_a_row_opens_its_actions_and_destructive_ones_confirm() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    let before = ui.state();
    ui.right_click(selector(format!("{DEPLOYMENTS}-row-1")))?;
    ui.click(selector(format!("{DEPLOYMENTS}-row-menu-item-2")))?;
    assert!(ui.bounds("dialog-cancel").is_some());
    ui.click("dialog-cancel")?;
    assert_eq!(ui.state(), before);
    ui.hover(selector(format!("{DEPLOYMENTS}-row-3")))?;
    ui.click(selector(format!("{DEPLOYMENTS}-row-3-more")))?;
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-menu-item-0")))
            .is_some()
    );
    ui.press("escape")?;
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-menu-item-0")))
            .is_none()
    );
    Ok(())
}

#[test]
fn right_clicking_a_row_works_after_the_palette_closes() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    ui.press("cmd-k")?;
    ui.type_text("nodes")?;
    ui.press("escape")?;
    ui.right_click(selector(format!("{DEPLOYMENTS}-row-1")))?;
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-menu-item-0")))
            .is_some()
    );
    Ok(())
}

#[test]
fn a_closing_palette_does_not_run_the_hit_under_a_click() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-shift-p")?;
    ui.type_text("toggle sidebar")?;
    let hit = ui
        .bounds("palette-hit-0")
        .ok_or_else(|| anyhow::anyhow!("the palette shows no hit"))?;
    ui.press("escape")?;
    ui.click_at(hit.center(), 1)?;
    assert!(ui.state().sidebar_open);
    Ok(())
}

#[test]
fn command_p_quick_opens_a_page_and_command_shift_p_runs_a_command() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-p")?;
    ui.type_text("nodes")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::Nodes);
    assert!(ui.state().sidebar_open);
    ui.press("cmd-shift-p")?;
    ui.type_text("toggle sidebar")?;
    ui.press("enter")?;
    assert!(!ui.state().sidebar_open);
    Ok(())
}

#[test]
fn back_and_forward_walk_the_visited_pages() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.press("cmd-p")?;
    ui.type_text("nodes")?;
    ui.press("enter")?;
    ui.press("cmd-p")?;
    ui.type_text("events")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::Events);
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, Page::Nodes);
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.press("cmd-]")?;
    assert_eq!(ui.state().location().page, Page::Nodes);
    ui.press("cmd-] cmd-]")?;
    assert_eq!(ui.state().location().page, Page::Events);
    Ok(())
}

#[test]
fn arrow_keys_home_end_and_type_ahead_move_through_a_table() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    let clusters: Vec<ClusterId> = ui
        .state()
        .snapshot
        .clusters
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let (Some(first), Some(second), Some(last)) =
        (clusters.first(), clusters.get(1), clusters.last())
    else {
        anyhow::bail!("the demo has too few clusters");
    };
    let mut open = |keys: &str| -> anyhow::Result<Option<ClusterId>> {
        ui.press(&format!("cmd-e {keys} enter"))?;
        let cluster = ui.state().location().cluster.clone();
        ui.press("cmd-[")?;
        assert_eq!(ui.state().location().page, Page::Overview);
        Ok(cluster)
    };
    assert_eq!(open("down")?.as_ref(), Some(first));
    assert_eq!(open("home down")?.as_ref(), Some(second));
    assert_eq!(open("end")?.as_ref(), Some(last));
    assert_eq!(open("end up home")?.as_ref(), Some(first));
    assert_eq!(open("home pagedown")?.as_ref(), Some(last));
    assert_eq!(open("end pageup")?.as_ref(), Some(first));
    ui.press("cmd-e home")?;
    ui.type_text("stag")?;
    ui.press("enter")?;
    assert_eq!(
        ui.state().location().cluster,
        Some(ClusterId::new("staging"))
    );
    Ok(())
}

#[test]
fn escape_closes_the_inspector_and_keeps_the_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Nodes)?;
    ui.press("cmd-e down")?;
    assert!(ui.bounds("node-inspector-pane").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("node-inspector-pane").is_none());
    assert_eq!(ui.state().location().page, Page::Nodes);
    Ok(())
}

#[test]
fn escape_closes_the_panel_from_a_panel_section_too() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Nodes)?;
    ui.click("nodes-row-0")?;
    ui.click("node-usage-CPU")?;
    assert!(ui.bounds("node-inspector-pane").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("node-inspector-pane").is_none());
    assert_eq!(ui.state().location().page, Page::Nodes);
    Ok(())
}

#[test]
fn escape_closes_the_pods_panel_and_enter_or_a_click_reopens_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.click("pods-row-1")?;
    let row = bounds_of(&mut ui, "pods-row-1")?;
    assert!(ui.bounds("pod-inspector-pane").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("pod-inspector-pane").is_none());
    assert_eq!(bounds_of(&mut ui, "pods-row-1")?.origin.y, row.origin.y);
    assert_eq!(ui.state().location().page, Page::Pods);
    ui.press("enter")?;
    assert!(ui.bounds("pod-inspector-pane").is_some());
    ui.click("pod-container-head")?;
    ui.press("escape")?;
    assert!(ui.bounds("pod-inspector-pane").is_none());
    ui.click("pods-row-2")?;
    assert!(ui.bounds("pod-inspector-pane").is_some());
    Ok(())
}

#[test]
fn escape_closes_a_menu_or_the_palette_before_the_pods_panel() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.click("pods-row-0")?;
    ui.press("cmd-k escape")?;
    assert!(ui.bounds("pod-inspector-pane").is_some());
    ui.right_click("pods-row-1")?;
    assert!(ui.bounds("pods-row-menu-item-0").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("pods-row-menu-item-0").is_none());
    assert!(ui.bounds("pod-inspector-pane").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("pod-inspector-pane").is_none());
    Ok(())
}

#[test]
fn escape_goes_to_a_running_terminal_and_closes_the_panel_once_it_ends() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::PodsShell)?;
    ui.click("pod-open-shell")?;
    ui.press("escape")?;
    assert!(ui.bounds("pod-inspector-pane").is_some());
    assert!(ui.bounds("pod-close-shell").is_some());
    ui.click("pod-close-shell")?;
    ui.click("pods-row-0")?;
    ui.press("escape")?;
    assert!(ui.bounds("pod-inspector-pane").is_none());
    Ok(())
}

#[test]
fn command_j_toggles_the_dock() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    let dock = |ui: &Ui| ui.read(Shell::dock_open);
    assert_eq!(dock(&ui), Some(true));
    ui.press("cmd-j")?;
    assert_eq!(dock(&ui), Some(false));
    ui.press("cmd-j")?;
    assert_eq!(dock(&ui), Some(true));
    assert!(ui.state().workload().is_some());
    Ok(())
}

#[test]
fn command_shift_e_focuses_the_sidebar_where_arrows_and_enter_pick_a_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-shift-e down down enter")?;
    assert_eq!(ui.state().location().page, Page::Namespaces);
    ui.press("cmd-shift-e end enter")?;
    let last = ui.read(|shell, _| {
        shell
            .state()
            .snapshot
            .nav_sections(shell.state().location().cluster.as_ref())
            .last()
            .and_then(|s| s.pages.last().copied())
    });
    assert_eq!(Some(ui.state().location().page), last);
    ui.press("cmd-shift-e home")?;
    ui.type_text("ev")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::Events);
    Ok(())
}

#[test]
fn event_row_opens_inspector_enter_reopens_and_escape_closes() -> anyhow::Result<()> {
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::Events, flock::Preset::default(), appearance)?;
        assert!(ui.bounds("close-event").is_none());
        ui.click("events-row-0")?;
        assert!(ui.bounds("close-event").is_some());
        assert!(ui.bounds("event-object").is_some());
        ui.press("escape")?;
        assert!(ui.bounds("close-event").is_none());
        ui.press("down enter")?;
        assert!(ui.bounds("close-event").is_some());
        ui.click("event-object")?;
        assert_eq!(ui.state().location().page, Page::Pods);
    }
    Ok(())
}

#[test]
fn inspector_edge_resizes_clamps_and_double_click_resets() -> anyhow::Result<()> {
    use flock_ui::theme::{INSPECTOR_MAX, INSPECTOR_MIN};
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    let width = |ui: &mut Ui| ui.bounds("resource-inspector-pane").map(|b| b.size.width);
    let original = width(&mut ui).ok_or_else(|| anyhow::anyhow!("no inspector"))?;
    ui.drag("resource-inspector-resize", point(px(-120.), px(0.)))?;
    assert_eq!(width(&mut ui), Some(original + px(120.)));
    assert_eq!(
        ui.read(|_, cx| flock_ui::settings::current(cx)
            .inspector_widths
            .get("resource-inspector")
            .copied()),
        Some(600)
    );
    ui.click("close-resource")?;
    assert!(ui.bounds("resource-inspector-pane").is_none());
    ui.click("resources-ConfigMap-row-0")?;
    assert_eq!(width(&mut ui), Some(original + px(120.)));
    ui.drag("resource-inspector-resize", point(px(-2000.), px(0.)))?;
    assert_eq!(width(&mut ui), Some(INSPECTOR_MAX));
    ui.drag("resource-inspector-resize", point(px(2000.), px(0.)))?;
    assert_eq!(width(&mut ui), Some(INSPECTOR_MIN));
    ui.double_click("resource-inspector-resize")?;
    assert_eq!(width(&mut ui), Some(original));
    Ok(())
}

fn drag_widens_and_persists(
    ui: &mut Ui,
    pane: &'static str,
    handle: &'static str,
    by: f32,
) -> anyhow::Result<()> {
    use gpui::{point, px};
    let original = bounds_of(ui, pane)?.size.width;
    ui.drag(handle, point(px(by), px(0.)))?;
    let widened = original + px(by.abs());
    assert_eq!(bounds_of(ui, pane)?.size.width, widened);
    let kind = pane.trim_end_matches("-pane");
    let saved = ui.read(|_, cx| {
        flock_ui::settings::current(cx)
            .inspector_widths
            .get(kind)
            .copied()
    });
    assert_eq!(saved.map(|width| px(f32::from(width))), Some(widened));
    Ok(())
}

#[test]
fn two_pane_splits_resize_from_their_divider_and_remember_the_width() -> anyhow::Result<()> {
    const WIDER: f32 = 120.;
    let mut problems = Ui::open(Screen::Problems)?;
    drag_widens_and_persists(
        &mut problems,
        "problem-causes-pane",
        "problem-causes-resize",
        WIDER,
    )?;
    let mut settings = Ui::open(Screen::ClusterSettings)?;
    drag_widens_and_persists(
        &mut settings,
        "cluster-inspector-pane",
        "cluster-inspector-resize",
        -WIDER,
    )?;
    Ok(())
}

#[test]
fn fleet_problems_filter_by_severity_and_open_the_inspected_object() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::FleetProblems)?;
    assert_eq!(ui.state().location().page, Page::Problems);
    assert!(ui.bounds("fleet-problem-pane").is_some());
    assert!(ui.bounds("error-block-0").is_some());
    let idle = ui.bounds("fleet-problem-filter-field");
    ui.click("fleet-problem-filter-chips-3")?;
    assert_eq!(ui.bounds("fleet-problem-filter-field"), idle);
    assert!(ui.bounds("fleet-problems-row-2").is_none());
    ui.click("fleet-problems-row-1")?;
    ui.click("fleet-problem-open")?;
    let location = ui.state().location().clone();
    assert_eq!(
        location.flux_object.map(|key| key.name),
        Some("apps-payments".to_owned())
    );
    Ok(())
}

#[test]
fn events_namespace_choice_is_kept_per_cluster_in_preferences() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Events)?;
    let cluster = ui
        .state()
        .location()
        .cluster
        .clone()
        .ok_or_else(|| anyhow::anyhow!("cluster"))?;
    ui.click("events-namespace")?;
    ui.click("events-namespace-option-0")?;
    let state = ui.state();
    let preferences = &state
        .snapshot
        .catalog
        .get(&cluster)
        .ok_or_else(|| anyhow::anyhow!("settings"))?
        .preferences;
    assert_eq!(
        preferences.namespaces.get(EVENT_LIST),
        Some(&flock_core::settings::NamespaceSelection::All)
    );
    ui.press("cmd-shift-c")?;
    ui.type_text("staging")?;
    ui.press("enter")?;
    let other = ui.state();
    let other_preferences = &other
        .snapshot
        .catalog
        .get(&ClusterId::new("staging"))
        .ok_or_else(|| anyhow::anyhow!("other settings"))?
        .preferences;
    assert_eq!(other_preferences.namespaces.get(EVENT_LIST), None);
    ui.press("cmd-shift-c")?;
    ui.type_text(cluster.as_str())?;
    ui.press("enter")?;
    let restored = ui.state();
    assert_eq!(
        restored
            .snapshot
            .catalog
            .get(&cluster)
            .and_then(|settings| settings.preferences.namespaces.get(EVENT_LIST)),
        Some(&flock_core::settings::NamespaceSelection::All)
    );
    Ok(())
}

#[test]
fn yaml_wrap_and_expand_work_and_escape_returns() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    assert!(ui.bounds("code-horizontal").is_some());
    ui.click("code-wrap")?;
    assert!(ui.bounds("code-horizontal").is_none());
    assert!(ui.bounds("code-wrapped").is_some());
    ui.click("code-expand")?;
    let expanded = ui
        .bounds("expanded-yaml")
        .ok_or_else(|| anyhow::anyhow!("no expanded YAML"))?;
    let inspector = ui
        .bounds("resource-inspector-pane")
        .ok_or_else(|| anyhow::anyhow!("no inspector"))?;
    assert!(expanded.size.width > inspector.size.width);
    ui.press("escape")?;
    assert!(ui.bounds("expanded-yaml").is_none());
    assert!(ui.bounds("code-wrapped").is_some());
    ui.click("code-wrap")?;
    assert!(ui.bounds("code-horizontal").is_some());
    Ok(())
}

fn picked(ui: &Ui, cluster: &str, list: &str) -> Option<flock_core::settings::NamespaceSelection> {
    ui.state()
        .snapshot
        .catalog
        .get(&ClusterId::new(cluster))
        .and_then(|settings| settings.preferences.namespaces.get(list).cloned())
}

fn open_deployments(ui: &mut Ui, cluster: &str) -> anyhow::Result<()> {
    ui.press("cmd-shift-c")?;
    ui.type_text(cluster)?;
    ui.press("enter")?;
    ui.press("cmd-k")?;
    ui.type_text("deployments")?;
    ui.press("enter")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    Ok(())
}

#[test]
fn yaml_scrolls_in_both_axes_and_the_gutter_stays_fixed_horizontally() -> anyhow::Result<()> {
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    let gutter = ui
        .bounds("code-gutter")
        .ok_or_else(|| anyhow::anyhow!("no gutter"))?;
    let lines = ui
        .bounds("code-lines")
        .ok_or_else(|| anyhow::anyhow!("no code"))?;
    ui.scroll("code-horizontal", point(px(-120.), px(0.)))?;
    let scrolled = ui
        .bounds("code-lines")
        .ok_or_else(|| anyhow::anyhow!("no code"))?;
    assert!(scrolled.origin.x < lines.origin.x);
    assert_eq!(ui.bounds("code-gutter"), Some(gutter));
    ui.scroll("code-vertical", point(px(0.), px(-120.)))?;
    let scrolled_gutter = ui
        .bounds("code-gutter")
        .ok_or_else(|| anyhow::anyhow!("no gutter"))?;
    assert!(scrolled_gutter.origin.y < gutter.origin.y);
    assert_eq!(scrolled_gutter.origin.x, gutter.origin.x);
    Ok(())
}

#[test]
fn overview_sections_navigate_to_their_lists() -> anyhow::Result<()> {
    for (selector, page, below) in [
        ("overview-pods", Page::Pods, false),
        ("overview-nodes", Page::Nodes, false),
        ("overview-pool-0", Page::Nodes, false),
        ("overview-fact-kubelet-skew", Page::Nodes, true),
        (
            "overview-fact-services-without-endpoints",
            Page::Services,
            true,
        ),
        ("overview-fact-load-balancers-pending", Page::Services, true),
        ("overview-fact-pvcs-pending", Page::Pvcs, true),
        ("overview-deployments", Page::Deployments, true),
        ("overview-statefulsets", Page::StatefulSets, true),
        ("overview-daemonsets", Page::DaemonSets, true),
        ("overview-jobs", Page::Jobs, true),
        ("overview-cronjobs", Page::CronJobs, true),
    ] {
        let mut ui = Ui::open(Screen::Cluster)?;
        let cluster = ui.state().location().cluster.clone();
        if below {
            scroll_overview_to_the_end(&mut ui)?;
        }
        ui.click(selector)?;
        assert_eq!(ui.state().location().page, page);
        assert_eq!(ui.state().location().cluster, cluster);
    }
    Ok(())
}

const SHAPING_TOLERANCE: f32 = 2.;
const OVERVIEW_ERRORS: &str = "✕ 2";
const OVERVIEW_WARNINGS: &str = "▲ 3";

fn shaped_width(ui: &mut Ui, parts: &[(&str, &str)]) -> Pixels {
    let text: String = parts.iter().map(|(_, part)| *part).collect();
    let runs: Vec<TextRun> = parts
        .iter()
        .map(|(family, part)| TextRun {
            len: part.len(),
            font: font(*family),
            color: rgb(0).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        })
        .collect();
    ui.read_in_window(|_, window, _| {
        window
            .text_system()
            .shape_line(SharedString::from(text), TEXT_MONO, &runs, None)
            .width
    })
}

#[test]
fn the_overview_problem_counts_draw_their_symbols_in_the_status_font() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let part = |text: &'static str| {
        let (symbol, rest) =
            text.split_at(text.char_indices().nth(1).map_or(text.len(), |(i, _)| i));
        [(FONT_STATUS, symbol), (FONT_MONO, rest)]
    };
    let errors = shaped_width(&mut ui, &part(OVERVIEW_ERRORS));
    let warnings = shaped_width(&mut ui, &part(OVERVIEW_WARNINGS));
    let link = width(&mut ui, "overview-problems").context("no problems link")?;
    let expected = errors + warnings + SPACE_5;
    assert!(
        (link - expected).abs() < gpui::px(SHAPING_TOLERANCE),
        "{link:?} != {expected:?}"
    );
    Ok(())
}

#[test]
fn the_overview_header_has_no_live_toggle() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds("overview-ns-sort-0").is_some());
    assert!(ui.bounds("overview-live").is_none());
    Ok(())
}

#[test]
fn overview_expiring_follows_the_horizon_and_opens_certificates() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    scroll_overview_to_the_end(&mut ui)?;
    assert!(ui.bounds("overview-expiring-2").is_some());
    assert!(ui.bounds("overview-expiring-3").is_none());
    assert!(ui.bounds("overview-fact-support").is_none());
    ui.click("overview-expiring-horizon-3")?;
    assert!(ui.bounds("overview-expiring-3").is_some());
    assert!(ui.bounds("overview-expiring-more").is_some());
    assert!(ui.bounds("overview-fact-support").is_some());
    ui.click("overview-expiring-horizon-0")?;
    assert!(ui.bounds("overview-expiring-1").is_some());
    assert!(ui.bounds("overview-expiring-2").is_none());
    assert!(ui.bounds("overview-fact-kubelet-skew").is_some());
    ui.click("overview-expiring-0")?;
    assert_eq!(
        opened(&ui),
        Some((
            "Certificate.cert-manager.io".to_owned(),
            "wildcard-onloco".to_owned()
        ))
    );
    Ok(())
}

#[test]
fn overview_sections_fill_their_cells_between_the_rules() -> anyhow::Result<()> {
    use flock_ui::theme::RULE;

    let mut ui = Ui::open(Screen::Cluster)?;
    let pods = ui
        .bounds("overview-pods")
        .ok_or_else(|| anyhow::anyhow!("missing pods section"))?;
    let nodes = ui
        .bounds("overview-nodes")
        .ok_or_else(|| anyhow::anyhow!("missing nodes section"))?;
    let memory = ui
        .bounds("overview-capacity-Memory")
        .ok_or_else(|| anyhow::anyhow!("missing memory meter"))?;
    assert_eq!(pods.size.height, nodes.size.height);
    assert_eq!(pods.origin.y, nodes.origin.y);
    assert_eq!(nodes.origin.x + nodes.size.width + RULE, pods.origin.x);
    assert!(memory.origin.x > pods.origin.x);
    Ok(())
}

#[test]
fn overview_sections_accept_keyboard_activation() -> anyhow::Result<()> {
    for key in ["enter", "space"] {
        let mut ui = Ui::open(Screen::Cluster)?;
        ui.press("cmd-e")?;
        ui.press(key)?;
        assert_eq!(ui.state().location().page, Page::Nodes);
    }
    Ok(())
}

const OVERVIEW_SCROLL_TO_END: f32 = -4000.;

fn scroll_overview_to_the_end(ui: &mut Ui) -> anyhow::Result<()> {
    let delta = gpui::point(gpui::px(0.), gpui::px(OVERVIEW_SCROLL_TO_END));
    ui.scroll("overview-scroll", delta)
}

const OVERVIEW_NAMESPACES_TOP: f32 = 200.;

fn show_overview_namespaces(ui: &mut Ui) -> anyhow::Result<()> {
    let head = ui
        .bounds("overview-ns-sort-0")
        .context("the namespaces section is laid out")?;
    let delta = gpui::point(gpui::px(0.), gpui::px(OVERVIEW_NAMESPACES_TOP) - head.top());
    ui.scroll("overview-scroll", delta)
}

fn opened(ui: &Ui) -> Option<(String, String)> {
    ui.state()
        .location()
        .resource
        .as_ref()
        .map(|target| (target.kind.clone(), target.name.clone()))
}

fn needs_attention_in(
    ui: &Ui,
    page: Page,
    list: &str,
) -> Option<flock_core::settings::NamespaceSelection> {
    use flock_core::resources::attention_chip;
    let location = ui.state().location().clone();
    assert_eq!(location.page, page);
    let chip = ui.read(|shell, cx| shell.filter_bar(cx).map(|bar| bar.read(cx).chip()));
    assert_eq!(chip, Some(attention_chip(page)));
    let cluster = location.cluster?;
    let state = ui.state();
    let settings = state.snapshot.catalog.get(&cluster)?;
    Some(settings.preferences.namespace_selection(list))
}

const ALL_NAMESPACES: flock_core::settings::NamespaceSelection =
    flock_core::settings::NamespaceSelection::All;

#[test]
fn overview_problem_counts_open_their_list_on_the_rows_that_need_attention() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-pods-failing")?;
    assert_eq!(
        needs_attention_in(&ui, Page::Pods, "Pod"),
        Some(ALL_NAMESPACES)
    );
    assert!(ui.bounds("pods-row-0").is_some());
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-pending-meta")?;
    assert_eq!(
        needs_attention_in(&ui, Page::Pods, "Pod"),
        Some(ALL_NAMESPACES)
    );
    let mut ui = Ui::open(Screen::Cluster)?;
    show_overview_namespaces(&mut ui)?;
    ui.click("overview-ns-sort-2")?;
    ui.click("overview-ns-0")?;
    let namespace = opened(&ui);
    let mut ui = Ui::open(Screen::Cluster)?;
    show_overview_namespaces(&mut ui)?;
    ui.click("overview-ns-sort-2")?;
    ui.click("overview-ns-0-failing")?;
    assert!(namespace.is_some());
    assert_eq!(opened(&ui), namespace);
    let mut ui = Ui::open(Screen::Cluster)?;
    scroll_overview_to_the_end(&mut ui)?;
    ui.click("overview-deployments-bad")?;
    let all = needs_attention_in(&ui, Page::Deployments, "Deployment");
    assert_eq!(all, Some(ALL_NAMESPACES));
    assert!(ui.bounds("resources-Deployment-row-0").is_some());
    let mut ui = Ui::open(Screen::Cluster)?;
    scroll_overview_to_the_end(&mut ui)?;
    ui.click("overview-deployments")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    let chip = ui.read(|shell, cx| shell.filter_bar(cx).map(|bar| bar.read(cx).chip()));
    assert_eq!(chip, Some(0));
    Ok(())
}

#[test]
fn the_overview_opens_on_its_cards_then_the_cpu_and_memory_charts() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let cards = ui
        .bounds("overview-capacity-Memory")
        .context("the memory card renders")?;
    let charts = ui
        .bounds("utilization-chart")
        .context("the charts render")?;
    let namespaces = ui
        .bounds("overview-ns-0")
        .context("the namespaces render")?;
    assert!(cards.bottom() <= charts.top());
    assert!(charts.bottom() <= namespaces.top());
    Ok(())
}

#[test]
fn overview_namespace_headers_resort_the_top_namespaces() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    show_overview_namespaces(&mut ui)?;
    ui.click("overview-ns-0")?;
    assert_eq!(opened(&ui), Some(("Namespace".into(), "search".into())));
    let mut ui = Ui::open(Screen::Cluster)?;
    show_overview_namespaces(&mut ui)?;
    ui.click("overview-ns-sort-0")?;
    ui.click("overview-ns-0")?;
    assert_eq!(opened(&ui), Some(("Namespace".into(), "analytics".into())));
    let mut ui = Ui::open(Screen::Cluster)?;
    show_overview_namespaces(&mut ui)?;
    ui.click("overview-ns-sort-1")?;
    ui.click("overview-ns-0")?;
    assert_eq!(
        opened(&ui),
        Some(("Namespace".into(), "kube-system".into()))
    );
    let mut ui = Ui::open(Screen::Cluster)?;
    show_overview_namespaces(&mut ui)?;
    ui.click("overview-ns-all")?;
    assert_eq!(ui.state().location().page, Page::Namespaces);
    Ok(())
}

#[test]
fn overview_pod_tops_open_the_pod_and_link_to_the_pods_list() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-restarts-0")?;
    assert_eq!(
        opened(&ui),
        Some(("Pod".into(), "checkout-api-7f9c6d-x2lqp".into()))
    );
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-pending-0")?;
    assert_eq!(opened(&ui).map(|(kind, _)| kind), Some("Pod".into()));
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-pending-all")?;
    assert_eq!(ui.state().location().page, Page::Pods);
    Ok(())
}

#[test]
fn overview_recent_changes_open_the_object_and_link_to_changes() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-change-0")?;
    assert_eq!(
        opened(&ui),
        Some(("Deployment".into(), "checkout-api".into()))
    );
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-change-4")?;
    assert_eq!(opened(&ui), Some(("Node".into(), "ip-10-4-20-19".into())));
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds("overview-change-5").is_some());
    assert!(ui.bounds("overview-change-6").is_none());
    let cluster = ui.state().location().cluster.clone();
    ui.click("overview-changes-all")?;
    assert_eq!(ui.state().location().page, Page::Changes);
    assert_eq!(ui.read(Shell::changes_cluster), cluster);
    assert!(cluster.is_some());
    Ok(())
}

#[test]
fn overview_header_links_to_the_clusters_problems_and_flux() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let cluster = ui.state().location().cluster.clone();
    ui.click("overview-problems")?;
    assert_eq!(ui.state().location().page, Page::ClusterProblems);
    assert_eq!(ui.state().location().cluster, cluster);
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("overview-flux")?;
    assert_eq!(ui.state().location().page, Page::Kustomizations);
    Ok(())
}

#[test]
fn selected_nav_rows_fill_the_panel_between_the_design_insets() -> anyhow::Result<()> {
    use flock_ui::theme::{ROW_NAV, SIDEBAR, SPACE_4};

    let mut ui = Ui::open(Screen::Cluster)?;
    let overview = ui
        .bounds("nav-overview")
        .ok_or_else(|| anyhow::anyhow!("missing overview row"))?;
    ui.click("nav-nodes")?;
    let nodes = ui
        .bounds("nav-nodes")
        .ok_or_else(|| anyhow::anyhow!("missing nodes row"))?;
    assert_eq!(nodes.origin.x, overview.origin.x);
    assert_eq!(nodes.size.width, SIDEBAR - SPACE_4 - SPACE_4);
    assert_eq!(nodes.size.width, overview.size.width);
    assert_eq!(nodes.size.height, ROW_NAV);
    Ok(())
}

fn sidebar_highlight(ui: &mut Ui) -> Page {
    ui.read_in_window(|shell, window, _| shell.sidebar_highlight(window))
}

#[test]
fn the_sidebar_highlight_follows_the_shown_page_after_keyboard_navigation() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    ui.press("cmd-shift-e down enter")?;
    let first = ui.state().location().page;
    assert_ne!(first, Page::Nodes);
    assert_eq!(sidebar_highlight(&mut ui), first);
    ui.click("nav-nodes")?;
    assert_eq!(ui.state().location().page, Page::Nodes);
    assert_eq!(sidebar_highlight(&mut ui), Page::Nodes);
    ui.press("cmd-shift-e down")?;
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, first);
    assert_eq!(sidebar_highlight(&mut ui), first);
    Ok(())
}

#[test]
fn node_inspector_leads_with_usage_and_keeps_labels_and_conditions_collapsed() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Nodes)?;
    ui.click("nodes-row-0")?;
    let usage = top(&mut ui, "node-usage-CPU")?;
    let memory = top(&mut ui, "node-usage-Memory")?;
    let disk = top(&mut ui, "node-usage-Disk")?;
    let pods = top(&mut ui, "node-usage-Pods")?;
    assert!(usage < memory && memory < disk && disk < pods);
    let details = top(&mut ui, "node-inspector-details")?;
    assert!(usage < details);
    assert!(ui.bounds("node-inspector-labels-body").is_none());
    assert!(ui.bounds("node-inspector-conditions-body").is_none());
    assert!(ui.bounds("node-inspector-pods-body").is_some());
    ui.click("node-inspector-conditions-toggle")?;
    assert!(ui.bounds("condition-table").is_some());
    Ok(())
}

#[test]
fn node_inspector_keeps_the_disk_row_when_the_kubelet_summary_is_unavailable() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Nodes)?;
    ui.click("nodes-row-0")?;
    let measured = ui.bounds("node-usage-Disk").map(|b| b.size);
    ui.click("nodes-row-13")?;
    let unavailable = ui.bounds("node-usage-Disk").map(|b| b.size);
    assert!(measured.is_some());
    assert_eq!(measured, unavailable);
    Ok(())
}

#[test]
fn node_filters_narrow_by_pool_and_status_chip() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Nodes)?;
    assert!(ui.bounds("nodes-row-13").is_some());
    ui.click("node-pool")?;
    ui.click("node-pool-option-2")?;
    assert!(ui.bounds("nodes-row-1").is_some());
    assert!(ui.bounds("nodes-row-2").is_none());
    ui.click("node-pool")?;
    ui.click("node-pool-option-0")?;
    ui.click("node-filter-chips-3")?;
    assert!(ui.bounds("nodes-row-0").is_some());
    assert!(ui.bounds("nodes-row-1").is_none());
    Ok(())
}

#[test]
fn the_terminal_tab_waits_for_an_open_shell_click() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("dock-tab-2")?;
    assert!(ui.bounds("workload-open-shell").is_some());
    assert!(ui.bounds("workload-close-shell").is_none());
    let mut pods = Ui::open(Screen::PodsShell)?;
    assert!(pods.bounds("pod-open-shell").is_some());
    assert!(pods.bounds("pod-close-shell").is_none());
    Ok(())
}

#[test]
fn the_pod_shell_button_starts_a_shell_in_the_terminal_tab() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.click("pod-shell")?;
    assert!(ui.bounds("pod-close-shell").is_some());
    ui.click("pod-shell")?;
    assert!(ui.bounds("pod-close-shell").is_some());
    ui.type_text("exit")?;
    ui.press("enter")?;
    assert!(ui.bounds("pod-open-shell").is_some());
    Ok(())
}

#[test]
fn the_workload_shell_action_and_shortcut_start_a_shell() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("workload-actions")?;
    ui.click("workload-actions-item-2")?;
    assert!(ui.bounds("workload-close-shell").is_some());
    ui.click("workload-close-shell")?;
    ui.click("dock-tab-0")?;
    ui.press("ctrl-`")?;
    assert!(ui.bounds("workload-close-shell").is_some());
    Ok(())
}

#[test]
fn flux_pages_list_their_objects_and_open_one_on_click() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    for page in [Page::Kustomizations, Page::HelmReleases, Page::Sources] {
        let selector: &'static str = format!("nav-{}", page.label().to_lowercase()).leak();
        ui.click(selector)?;
        assert_eq!(ui.state().location().page, page);
        assert!(
            ui.bounds("flux-list-row-0").is_some(),
            "{page:?} shows no Flux objects"
        );
    }
    ui.click("flux-list-row-0")?;
    let opened = ui.state().location().flux_object.clone();
    assert_eq!(opened.map(|key| key.kind.page()), Some(Page::Sources));
    Ok(())
}

#[test]
fn keys_typed_in_an_open_shell_reach_it_including_tab_arrows_and_control_c() -> anyhow::Result<()> {
    for (screen, open, close) in [
        (
            Screen::Workload,
            "workload-open-shell",
            "workload-close-shell",
        ),
        (Screen::PodsShell, "pod-open-shell", "pod-close-shell"),
    ] {
        let mut ui = Ui::open(screen)?;
        if let Screen::Workload = screen {
            ui.click("dock-tab-2")?;
        }
        ui.click(open)?;
        assert!(ui.bounds(close).is_some());
        ui.type_text("whoami")?;
        ui.press("tab left up ctrl-c")?;
        ui.type_text("exit")?;
        ui.press("enter")?;
        assert!(ui.bounds(open).is_some(), "{open}: the shell did not exit");
        assert!(ui.bounds(close).is_none());
    }
    Ok(())
}

#[test]
fn close_shell_and_collapsing_the_dock_end_the_session_but_switching_tabs_keeps_it()
-> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("dock-tab-2")?;
    ui.click("workload-open-shell")?;
    ui.click("workload-close-shell")?;
    assert!(ui.bounds("workload-open-shell").is_some());
    ui.click("workload-open-shell")?;
    ui.click("dock-tab-0")?;
    ui.click("dock-tab-2")?;
    assert!(ui.bounds("workload-close-shell").is_some());
    ui.click("dock-toggle")?;
    ui.click("dock-toggle")?;
    assert!(ui.bounds("workload-open-shell").is_some());
    ui.click("workload-open-shell")?;
    ui.click("terminal-containers-1")?;
    assert!(ui.bounds("workload-open-shell").is_some());
    Ok(())
}

#[test]
fn logs_before_restart_open_the_previous_instance_with_a_banner() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    assert!(ui.bounds("pod-logs-raw").is_some());
    assert!(ui.bounds("pod-logs-previous-banner").is_none());
    let crashed = (0..POD_ROWS).find_map(|ix| {
        let row: &'static str = format!("pods-row-{ix}").leak();
        ui.click(row).ok()?;
        ui.bounds("pod-logs-before-restart-0").map(|_| ix)
    });
    assert!(crashed.is_some(), "no demo pod shows a last termination");
    assert!(ui.bounds("pod-logs-instance-1").is_some());
    ui.press("cmd-j")?;
    assert_eq!(ui.read(Shell::dock_open), Some(false));
    ui.scroll(
        "pod-inspector-pane",
        gpui::point(gpui::px(0.), gpui::px(-INSPECTOR_SCROLL)),
    )?;
    ui.click("pod-logs-before-restart-0")?;
    assert_eq!(ui.read(Shell::dock_open), Some(true));
    assert!(ui.bounds("pod-logs-previous-banner").is_some());
    ui.click("pod-logs-raw")?;
    assert!(ui.bounds("pod-logs-previous-banner").is_some());
    ui.click("pod-logs-instance-0")?;
    assert!(ui.bounds("pod-logs-previous-banner").is_none());
    ui.click("pod-logs-instance-1")?;
    assert!(ui.bounds("pod-logs-previous-banner").is_some());
    Ok(())
}

#[test]
fn lists_start_in_the_default_namespace_and_remember_the_pick_per_cluster_and_kind()
-> anyhow::Result<()> {
    const BEYOND_ONE_NAMESPACE: &str = "resources-Deployment-row-8";
    let mut ui = Ui::open(Screen::Resources)?;
    assert_eq!(picked(&ui, SAMPLE, "Deployment"), None);
    assert!(ui.bounds(BEYOND_ONE_NAMESPACE).is_none());
    ui.click("resource-namespace")?;
    ui.click("resource-namespace-option-0")?;
    assert_eq!(
        picked(&ui, SAMPLE, "Deployment"),
        Some(flock_core::settings::NamespaceSelection::All)
    );
    assert_eq!(picked(&ui, SAMPLE, EVENT_LIST), None);
    assert!(ui.bounds(BEYOND_ONE_NAMESPACE).is_some());
    open_deployments(&mut ui, "staging")?;
    assert_eq!(picked(&ui, "staging", "Deployment"), None);
    assert!(ui.bounds(BEYOND_ONE_NAMESPACE).is_none());
    open_deployments(&mut ui, SAMPLE)?;
    assert!(ui.bounds(BEYOND_ONE_NAMESPACE).is_some());
    Ok(())
}

#[test]
fn every_page_state_renders_under_the_filter_bar_and_retry_reloads() -> anyhow::Result<()> {
    use flock_shots::StateFixture;
    let states = [
        (StateFixture::Loading, "loading-skeleton", None),
        (StateFixture::Connecting, "connection-progress", None),
        (StateFixture::Empty, "empty-state-cluster", None),
        (StateFixture::Error, "error-state", Some("page-retry")),
    ];
    for (state, selector, action) in states {
        let mut ui = Ui::with_state(Screen::Resources, state)?;
        assert!(
            ui.bounds(selector).is_some(),
            "{state:?} shows no {selector}"
        );
        assert!(
            ui.bounds("resource-filter").is_some(),
            "{state:?} hides the filter bar"
        );
        assert!(
            ui.bounds("page-empty-action").is_none(),
            "{state:?} offers a refresh"
        );
        if let Some(action) = action {
            ui.click(action)?;
            assert!(
                ui.bounds(selector).is_none(),
                "{state:?} stayed after {action}"
            );
            assert!(ui.bounds("resources-Deployment-row-0").is_some());
        }
    }
    Ok(())
}

fn namespace_without_deployments(ui: &Ui) -> anyhow::Result<usize> {
    let state = ui.state();
    let cluster = state
        .location()
        .cluster
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no cluster"))?;
    let detail = state
        .snapshot
        .detail(&cluster)
        .ok_or_else(|| anyhow::anyhow!("no detail"))?;
    let deployments = detail.resources(flock_core::ResourceKind::Deployment);
    let mut names: std::collections::BTreeSet<String> =
        detail.namespaces.iter().map(|n| n.name.clone()).collect();
    names.extend(deployments.iter().map(|d| d.namespace.clone()));
    names
        .iter()
        .position(|name| !deployments.iter().any(|d| &d.namespace == name))
        .ok_or_else(|| anyhow::anyhow!("every namespace has deployments"))
}

#[test]
fn a_namespace_without_rows_keeps_the_filter_bar_and_offers_all_namespaces() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    let ix = namespace_without_deployments(&ui)?;
    ui.click("resource-namespace")?;
    ui.click(selector(format!("resource-namespace-option-{}", ix + 1)))?;
    assert!(ui.bounds("resource-filter").is_some());
    assert!(ui.bounds("resource-namespace").is_some());
    assert!(ui.bounds("empty-state-namespace").is_some());
    assert!(ui.bounds("empty-state-cluster").is_none());
    assert!(ui.bounds("empty-clear-filter").is_none());
    ui.click("empty-show-all-namespaces")?;
    assert!(ui.bounds("empty-state-namespace").is_none());
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-0")))
            .is_some()
    );
    assert_eq!(
        picked(&ui, SAMPLE, "Deployment"),
        Some(flock_core::settings::NamespaceSelection::All)
    );
    Ok(())
}

#[test]
fn a_filter_that_hides_every_row_keeps_the_bar_and_clears_with_one_click() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    ui.press("/")?;
    ui.type_text("nothing-is-called-this")?;
    assert!(ui.bounds("resource-filter").is_some());
    assert!(ui.bounds("empty-state-filter").is_some());
    assert!(ui.bounds("empty-show-all-namespaces").is_none());
    ui.click("empty-clear-filter")?;
    assert!(ui.bounds("empty-state-filter").is_none());
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-0")))
            .is_some()
    );
    Ok(())
}

#[test]
fn row_menus_carry_each_objects_actions_and_inspectors_keep_only_the_primary() -> anyhow::Result<()>
{
    let mut nodes = Ui::open(Screen::Nodes)?;
    nodes.click("nodes-row-0")?;
    nodes.click("node-actions")?;
    assert!(nodes.bounds("node-actions-item-2").is_some());
    nodes.click("node-actions-item-1")?;
    assert!(nodes.bounds("dialog-cancel").is_some());
    nodes.click("dialog-cancel")?;
    nodes.right_click("nodes-row-1")?;
    assert!(nodes.bounds("nodes-row-menu-item-0").is_some());
    nodes.click("nodes-row-menu-item-3")?;
    assert!(nodes.bounds("dialog-cancel").is_some());
    nodes.click("dialog-cancel")?;

    let mut pods = Ui::open(Screen::Pods)?;
    assert!(pods.bounds("pod-shell").is_some());
    assert!(pods.bounds("pod-actions").is_none());
    pods.hover("pods-row-2")?;
    pods.click("pods-row-2-more")?;
    assert!(pods.bounds("pods-row-menu-item-0").is_some());

    let mut flock = Ui::open(Screen::Flock)?;
    flock.right_click("flock-clusters-row-2")?;
    flock.click("flock-clusters-row-menu-item-1")?;
    assert_eq!(flock.state().location().page, Page::ClusterSettings);
    assert_eq!(settings_selection(&flock), Some(cluster_at(&flock, 2)?.id));
    Ok(())
}

#[test]
fn list_pages_open_on_the_filter_without_a_summary_sentence() -> anyhow::Result<()> {
    for screen in [
        Screen::Resources,
        Screen::Nodes,
        Screen::Namespaces,
        Screen::Events,
    ] {
        let mut ui = Ui::open(screen)?;
        let filter = ui.read(|shell, cx| shell.filter_bar(cx).is_some());
        assert!(filter, "{screen:?} has no filter bar");
        assert!(ui.bounds("page-actions").is_none(), "{screen:?}");
    }
    Ok(())
}

fn tab_pages(ui: &Ui) -> Vec<Page> {
    ui.state()
        .panes()
        .focused()
        .tabs()
        .iter()
        .map(|tab| tab.location().page)
        .collect()
}

fn active_tab(ui: &Ui) -> usize {
    ui.state().panes().focused().active_index()
}

#[test]
fn keyboard_opens_switches_and_closes_tabs() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds("tab-0").is_some());
    ui.press("cmd-t")?;
    assert_eq!(tab_pages(&ui), [Page::ClusterOverview, Page::Overview]);
    assert_eq!(active_tab(&ui), 1);
    ui.press("cmd-t")?;
    assert_eq!(tab_pages(&ui).len(), 3);
    ui.press("cmd-1")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.press("ctrl-tab")?;
    assert_eq!(active_tab(&ui), 1);
    ui.press("ctrl-shift-tab ctrl-shift-tab")?;
    assert_eq!(active_tab(&ui), 2);
    ui.press("cmd-alt-right")?;
    assert_eq!(active_tab(&ui), 0);
    ui.press("cmd-alt-left")?;
    assert_eq!(active_tab(&ui), 2);
    ui.press("cmd-9")?;
    assert_eq!(active_tab(&ui), 2);
    ui.press("cmd-w cmd-w")?;
    assert_eq!(tab_pages(&ui), [Page::ClusterOverview]);
    assert_eq!(ui.read_app(|cx| cx.windows().len()), 1);
    ui.press("cmd-w")?;
    assert_eq!(ui.read_app(|cx| cx.windows().len()), 0);
    Ok(())
}

#[test]
fn sidebar_clicks_navigate_the_active_tab_and_command_or_middle_click_open_new_ones()
-> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes]);
    ui.cmd_click("nav-pods")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes, Page::Pods]);
    assert_eq!(active_tab(&ui), 1);
    ui.press("cmd-1")?;
    ui.cmd_click("nav-pods")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes, Page::Pods]);
    assert_eq!(active_tab(&ui), 1);
    ui.middle_click("nav-events")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes, Page::Pods, Page::Events]);
    ui.click("nav-namespaces")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes, Page::Pods, Page::Namespaces]);
    Ok(())
}

#[test]
fn the_tab_strip_switches_closes_and_reorders_with_the_mouse() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.cmd_click("nav-nodes")?;
    ui.cmd_click("nav-pods")?;
    assert_eq!(
        tab_pages(&ui),
        [Page::ClusterOverview, Page::Nodes, Page::Pods]
    );
    ui.click("tab-0")?;
    assert_eq!(active_tab(&ui), 0);
    assert!(ui.bounds("tab-close-0").is_some());
    let width = ui
        .bounds("tab-1")
        .map(|bounds| bounds.size.width)
        .ok_or_else(|| anyhow::anyhow!("tab-1 was not painted"))?;
    ui.drag("tab-0", gpui::point(width * 1.5, gpui::px(0.)))?;
    assert_eq!(
        tab_pages(&ui),
        [Page::Nodes, Page::ClusterOverview, Page::Pods]
    );
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.middle_click("tab-2")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes, Page::ClusterOverview]);
    ui.click("tab-close-1")?;
    assert_eq!(tab_pages(&ui), [Page::Nodes]);
    Ok(())
}

#[test]
fn each_tab_walks_its_own_history() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    ui.press("cmd-t")?;
    ui.click("nav-gitops")?;
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, Page::Overview);
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, Page::Overview);
    ui.press("cmd-1 cmd-[")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    Ok(())
}

#[test]
fn command_enter_in_the_palette_and_row_menus_open_new_tabs() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    ui.right_click(selector(format!("{DEPLOYMENTS}-row-1")))?;
    ui.click(selector(format!("{DEPLOYMENTS}-row-menu-item-0")))?;
    assert_eq!(tab_pages(&ui), [Page::Deployments, Page::Deployments]);
    assert_eq!(active_tab(&ui), 1);
    assert!(ui.state().workload().is_some());
    ui.press("cmd-1")?;
    ui.cmd_click(selector(format!("{DEPLOYMENTS}-row-1")))?;
    assert_eq!(tab_pages(&ui).len(), 2);
    assert_eq!(active_tab(&ui), 1);
    ui.press("cmd-k")?;
    ui.type_text("nodes")?;
    ui.press("cmd-enter")?;
    assert_eq!(
        tab_pages(&ui),
        [Page::Deployments, Page::Deployments, Page::Nodes]
    );
    assert_eq!(active_tab(&ui), 2);
    Ok(())
}

#[test]
fn the_workload_dock_opens_a_pods_logs_in_a_tab() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("logs-open-tab")?;
    let state = ui.state();
    let location = state.location();
    assert_eq!(location.page, Page::Logs);
    assert!(location.container.is_some());
    assert_eq!(
        tab_pages(&ui),
        [
            Page::Deployments,
            Page::Logs,
            Page::Kustomizations,
            Page::Deployments
        ]
    );
    assert_eq!(active_tab(&ui), 1);
    assert!(ui.bounds("tab-logs-follow").is_some());
    Ok(())
}

fn switcher_row(ui: &Ui, cluster: &str) -> anyhow::Result<&'static str> {
    let ix = ui
        .state()
        .snapshot
        .clusters
        .iter()
        .position(|c| c.id.as_str() == cluster)
        .ok_or_else(|| anyhow::anyhow!("the demo has no cluster {cluster}"))?;
    Ok(selector(format!("switcher-row-{}", ix + 1)))
}

#[test]
fn picking_a_cluster_retargets_the_active_tab_to_the_same_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-events")?;
    ui.click("cluster-switcher")?;
    let staging = switcher_row(&ui, "staging")?;
    ui.click(staging)?;
    let state = ui.state();
    assert_eq!(state.location().cluster, Some(ClusterId::new("staging")));
    assert_eq!(state.location().page, Page::Events);
    assert_eq!(tab_pages(&ui), [Page::Events]);
    ui.press("cmd-[")?;
    assert_eq!(
        ui.state().location().cluster,
        Some(ClusterId::new("prod-us-east"))
    );
    Ok(())
}

#[test]
fn a_workload_tab_follows_the_object_to_another_cluster_or_falls_back() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    let target = ui.state().workload().cloned();
    ui.click("cluster-switcher")?;
    let west = switcher_row(&ui, "prod-eu-west")?;
    ui.click(west)?;
    let moved = ui.state().workload().cloned();
    assert_eq!(
        moved
            .as_ref()
            .map(|r| (r.cluster.as_str(), r.name.as_str())),
        target.as_ref().map(|r| ("prod-eu-west", r.name.as_str()))
    );
    ui.press("cmd-shift-c")?;
    ui.type_text("edge")?;
    ui.press("enter")?;
    let state = ui.state();
    assert_eq!(state.location().cluster, Some(ClusterId::new("edge-k3s")));
    assert_eq!(state.location().page, Page::ClusterOverview);
    assert_eq!(state.panes().focused().tabs().len(), 3);
    Ok(())
}

#[test]
fn command_zero_shows_all_clusters_in_the_active_tab() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    ui.press("cmd-0")?;
    let state = ui.state();
    assert_eq!(state.location().cluster, None);
    assert_eq!(state.location().page, Page::Overview);
    assert_eq!(tab_pages(&ui), [Page::Overview]);
    Ok(())
}

#[test]
fn the_switcher_gear_and_right_click_open_the_cluster_menu_beside_the_list() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    ui.click("cluster-switcher")?;
    let staging = switcher_row(&ui, "staging")?;
    ui.right_click(staging)?;
    let menu = ui
        .bounds("switcher-menu")
        .context("right-click opens the menu")?;
    let list = ui.bounds(staging).context("the list stays open")?;
    assert!(menu.right() < list.left());
    assert!(ui.bounds("switcher-action-4").is_some());
    assert!(ui.bounds("switcher-action-5").is_none());
    ui.press("escape escape")?;
    assert!(!ui.read(Shell::switcher_open));
    ui.click("cluster-switcher")?;
    ui.hover(staging)?;
    let gear: &'static str = staging.replace("row", "gear").leak();
    ui.click(gear)?;
    assert!(ui.read(Shell::switcher_open));
    ui.click("switcher-action-0")?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    assert_eq!(settings_selection(&ui), Some(ClusterId::new("staging")));
    assert!(!ui.read(Shell::switcher_open));
    Ok(())
}

fn open_staging_action(ui: &mut Ui, action: &'static str) -> anyhow::Result<()> {
    ui.click("cluster-switcher")?;
    let staging = switcher_row(ui, "staging")?;
    ui.hover(staging)?;
    let gear: &'static str = staging.replace("row", "gear").leak();
    ui.click(gear)?;
    ui.click(action)
}

fn staging_name(ui: &Ui) -> Option<String> {
    let clusters = ui.state().snapshot.clusters;
    clusters
        .iter()
        .find(|c| c.id.as_str() == "staging")
        .map(|c| c.name.clone())
}

#[test]
fn the_gear_menu_renames_a_cluster_and_an_empty_name_resets_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    open_staging_action(&mut ui, "switcher-action-1")?;
    assert!(ui.bounds("rename-cluster").is_some());
    ui.press("cmd-a")?;
    ui.type_text("Stage")?;
    ui.press("escape")?;
    assert!(ui.bounds("rename-cluster").is_none());
    assert_eq!(staging_name(&ui).as_deref(), Some("staging"));
    open_staging_action(&mut ui, "switcher-action-1")?;
    ui.press("cmd-a")?;
    ui.type_text("Stage")?;
    ui.press("enter")?;
    assert!(ui.bounds("rename-cluster").is_none());
    assert_eq!(staging_name(&ui).as_deref(), Some("Stage"));
    assert_eq!(
        ui.state()
            .snapshot
            .cluster(&ClusterId::new("staging"))
            .map(|c| c.id.clone()),
        Some(ClusterId::new("staging"))
    );
    open_staging_action(&mut ui, "switcher-action-1")?;
    ui.press("cmd-a backspace enter")?;
    assert_eq!(staging_name(&ui).as_deref(), Some("staging"));
    Ok(())
}

#[test]
fn the_gear_menu_switches_a_cluster_to_another_context() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    open_staging_action(&mut ui, "switcher-action-2")?;
    assert!(ui.bounds("switch-context").is_some());
    assert!(ui.bounds("switch-context-row-2").is_some());
    assert!(ui.bounds("switch-context-row-3").is_none());
    ui.click("switch-context-row-0")?;
    assert!(ui.bounds("switch-context").is_none());
    assert_eq!(staging_name(&ui).as_deref(), Some("staging"));
    open_staging_action(&mut ui, "switcher-action-2")?;
    assert!(ui.bounds("switch-context-row-1").is_some());
    assert!(ui.bounds("switch-context-row-2").is_none());
    ui.press("escape")?;
    assert!(ui.bounds("switch-context").is_none());
    Ok(())
}

#[test]
fn the_sidebar_names_its_scope_and_hides_it_when_collapsed() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds("sidebar-scope").is_some());
    ui.press("cmd-b")?;
    assert!(ui.bounds("sidebar-scope").is_none());
    assert!(ui.bounds("expand-sidebar").is_some());
    Ok(())
}

struct GatedSource {
    gate: async_channel::Receiver<()>,
}

impl flock_update::Source for GatedSource {
    fn releases(&self, _: bool) -> flock_update::Result<Vec<flock_update::release::Release>> {
        self.gate
            .recv_blocking()
            .map_err(|error| flock_update::Error::Network(error.to_string()))?;
        Ok(Vec::new())
    }

    fn fetch(&self, _: &str, _: u64) -> flock_update::Result<Vec<u8>> {
        Err(flock_update::Error::Http(404))
    }
}

fn menu_action(name: &str) -> anyhow::Result<Box<dyn gpui::Action>> {
    flock::menus()
        .into_iter()
        .flat_map(|menu| menu.items)
        .find_map(|item| match item {
            gpui::MenuItem::Action {
                name: label,
                action,
                ..
            } if label.as_ref() == name => Some(action),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("no {name} menu item"))
}

#[test]
fn check_for_updates_from_the_menu_shows_checking_then_up_to_date() -> anyhow::Result<()> {
    use flock_core::UpdaterState;
    use flock_ui::updater;

    let mut ui = Ui::open(Screen::Flock)?;
    let (open, gate) = async_channel::bounded(1);
    let config = flock_update::Config {
        current: flock_update::version::parse(updater::version())?,
        identifier: flock::APP_ID.to_owned(),
        bundle_name: format!("{}.app", flock::APP_NAME),
        target: flock_update::Target::Manual,
        public_key: flock_update::signature::PUBLIC_KEY.to_owned(),
        max_download: 1 << 20,
        max_text: 1 << 10,
    };
    let service = flock_update::Updater::new(GatedSource { gate }, config, Box::new(|_| Ok(())));
    let (requests, request_rx) = async_channel::unbounded();
    let (status_tx, statuses) = async_channel::unbounded();
    flock_update::spawn(service, request_rx, status_tx)?;
    ui.update_app(|cx| updater::install(requests, statuses, cx));
    assert!(ui.bounds(updater::STATUS_ITEM).is_none());

    ui.dispatch(menu_action(flock::CHECK_FOR_UPDATES_ITEM)?)?;
    let checking = ui.read_app(|cx| updater::status(cx).state.clone());
    assert_eq!(checking, UpdaterState::Checking);
    assert!(ui.bounds(updater::STATUS_ITEM).is_some());

    open.send_blocking(())?;
    ui.settle()?;
    let done = ui.read_app(|cx| updater::status(cx).clone());
    assert_eq!(done.state, UpdaterState::UpToDate);
    assert!(done.checked.is_some());
    let notices = ui.read_app(flock_ui::kit::toast_titles);
    assert_eq!(notices, ["Flock is up to date"]);
    assert!(ui.bounds(updater::STATUS_ITEM).is_none());
    Ok(())
}

#[test]
fn a_restored_tab_whose_cluster_is_gone_says_so() -> anyhow::Result<()> {
    use flock_core::history::Location;
    use flock_core::tabs::{SavedPane, SavedWindow};
    let mut ui = Ui::open(Screen::Flock)?;
    let gone = Location::page_on(ClusterId::new("decommissioned"), Page::Pods);
    let saved = SavedWindow {
        panes: vec![SavedPane {
            tabs: vec![Location::default(), gone.clone()],
            active: 1,
        }],
        focused: 0,
        ..SavedWindow::default()
    };
    ui.restore(&saved)?;
    assert_eq!(ui.state().location(), &gone);
    assert!(ui.bounds("cluster-unavailable").is_some());
    assert!(ui.bounds("tab-1").is_some());
    ui.click("tab-show-flock")?;
    assert_eq!(ui.state().location(), &Location::default());
    assert!(ui.bounds("cluster-unavailable").is_none());
    Ok(())
}

fn pane_count(ui: &Ui) -> usize {
    ui.state().panes().panes().len()
}

fn focused_pane(ui: &Ui) -> usize {
    ui.state().panes().focused_index()
}

fn pane_tabs(ui: &Ui, ix: usize) -> usize {
    ui.state()
        .panes()
        .panes()
        .get(ix)
        .map_or(0, |pane| pane.tabs().len())
}

fn center(ui: &mut Ui, selector: &'static str) -> anyhow::Result<gpui::Point<Pixels>> {
    ui.bounds(selector)
        .map(|bounds| bounds.center())
        .ok_or_else(|| anyhow::anyhow!("{selector} was not painted"))
}

#[test]
fn command_backslash_and_the_strip_button_split_the_active_tab_to_the_right() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Workload)?;
    let location = ui.state().location().clone();
    assert!(ui.bounds("split-right").is_some());
    ui.press("cmd-\\")?;
    assert_eq!(pane_count(&ui), 2);
    assert_eq!(focused_pane(&ui), 1);
    assert_eq!(ui.state().location(), &location);
    assert!(ui.bounds("pane-1-tab-0").is_some());
    assert!(ui.bounds("workload-full-view").is_some());
    ui.press("cmd-alt-1")?;
    assert_eq!(focused_pane(&ui), 0);
    ui.click("tab-1")?;
    ui.click("pane-1-split-right")?;
    assert_eq!(pane_count(&ui), 2);
    assert_eq!(focused_pane(&ui), 0);
    assert_eq!(pane_tabs(&ui, 0), 3);
    assert_eq!(ui.state().location(), &location);
    ui.press("cmd-alt-2")?;
    assert_eq!(focused_pane(&ui), 1);
    Ok(())
}

#[test]
fn clicking_a_pane_focuses_it_and_the_sidebar_follows_its_cluster() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Split)?;
    assert_eq!(pane_count(&ui), 2);
    assert_eq!(focused_pane(&ui), 0);
    let east = ui.state().location().cluster.clone();
    ui.click("pane-1")?;
    assert_eq!(focused_pane(&ui), 1);
    let west = ui.state().location().cluster.clone();
    assert_eq!(west, Some(ClusterId::new("prod-eu-west")));
    assert_ne!(east, west);
    ui.click("nav-pods")?;
    let right = ui.state().location().clone();
    assert_eq!((right.page, right.cluster), (Page::Pods, west));
    let left = ui
        .state()
        .panes()
        .panes()
        .first()
        .map(|pane| pane.active().location().clone());
    assert_eq!(left.and_then(|l| l.cluster), east);
    ui.click("tab-1")?;
    assert_eq!(focused_pane(&ui), 0);
    assert_eq!(ui.state().location().page, Page::Logs);
    Ok(())
}

#[test]
fn command_w_closes_the_focused_panes_tabs_and_then_the_pane() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Split)?;
    ui.click("pane-1-tab-1")?;
    ui.press("cmd-w")?;
    assert_eq!(pane_count(&ui), 2);
    assert_eq!(pane_tabs(&ui, 1), 1);
    ui.press("cmd-w")?;
    assert_eq!(pane_count(&ui), 1);
    assert_eq!(pane_tabs(&ui, 0), 3);
    assert!(ui.bounds("pane-1").is_none());
    assert!(ui.bounds("workload-full-view").is_none());
    Ok(())
}

#[test]
fn tabs_move_between_panes_by_menu_and_by_drag() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Split)?;
    ui.right_click("tab-2")?;
    ui.click("tab-menu-item-0")?;
    assert_eq!((pane_tabs(&ui, 0), pane_tabs(&ui, 1)), (2, 3));
    assert_eq!(focused_pane(&ui), 1);
    assert_eq!(ui.state().location().page, Page::Kustomizations);
    ui.scroll(
        "tab-strip-1",
        gpui::point(gpui::px(TAB_STRIP_REWIND), gpui::px(0.)),
    )?;
    let from = center(&mut ui, "pane-1-tab-0")?;
    let to = center(&mut ui, "tab-1")?;
    ui.drag("pane-1-tab-0", to - from)?;
    assert_eq!((pane_tabs(&ui, 0), pane_tabs(&ui, 1)), (3, 2));
    assert_eq!(focused_pane(&ui), 0);
    assert_eq!(
        ui.state().location().cluster,
        Some(ClusterId::new("prod-eu-west"))
    );
    ui.click("workload-full-view")?;
    assert_eq!(pane_count(&ui), 1);
    assert_eq!(pane_tabs(&ui, 0), 5);
    assert!(ui.bounds("workload-full-view").is_none());
    ui.right_click("tab-0")?;
    ui.click("tab-menu-item-0")?;
    assert_eq!((pane_tabs(&ui, 0), pane_tabs(&ui, 1)), (4, 1));
    Ok(())
}

#[test]
fn dragging_the_divider_writes_tabs_once_on_release() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Split)?;
    let writes = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let counted = writes.clone();
    ui.update_app(|cx| {
        let save: flock_ui::session::Saver = std::rc::Rc::new(move |_| {
            counted.set(counted.get() + 1);
            Ok(())
        });
        flock_ui::session::install(save, cx);
    });
    ui.drag("panes-divider", gpui::point(gpui::px(-200.), gpui::px(0.)))?;
    assert!(ui.state().panes().divider().share() < 0.5);
    assert_eq!(writes.get(), 1);
    Ok(())
}

#[test]
fn dragging_the_divider_resizes_the_panes_and_is_restored() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Split)?;
    let before = ui.bounds("pane-0").map(|b| b.size.width);
    ui.drag("panes-divider", gpui::point(gpui::px(-200.), gpui::px(0.)))?;
    let after = ui.bounds("pane-0").map(|b| b.size.width);
    assert!(after < before);
    let saved = ui.state().panes().saved();
    assert!(saved.divider.share() < 0.5);
    let mut again = Ui::open(Screen::Workload)?;
    again.restore(&saved)?;
    assert_eq!(pane_count(&again), 2);
    assert_eq!(again.state().panes().divider(), saved.divider);
    assert_eq!(again.bounds("pane-0").map(|b| b.size.width), after);
    Ok(())
}

#[test]
fn command_shift_enter_opens_a_palette_result_in_a_split() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-p")?;
    ui.type_text("Nodes")?;
    ui.press("cmd-shift-enter")?;
    assert_eq!(pane_count(&ui), 2);
    assert_eq!(focused_pane(&ui), 1);
    assert_eq!(ui.state().location().page, Page::Nodes);
    let left = ui
        .state()
        .panes()
        .panes()
        .first()
        .map(|pane| pane.active().location().page);
    assert_eq!(left, Some(Page::ClusterOverview));
    Ok(())
}

#[test]
fn split_panes_render_compact_logs_and_kustomization_views() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Split)?;
    ui.click("tab-1")?;
    ui.click("pane-1-tab-1")?;
    assert!(ui.bounds("flux-full-view").is_some());
    assert!(ui.bounds("tab-logs-follow").is_none());
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    ui.save_png(&dir.join("split-logs-kustomization.png"))?;
    ui.click("flux-full-view")?;
    assert_eq!(pane_count(&ui), 1);
    assert!(ui.bounds("flux-resume").is_some());
    Ok(())
}

fn bounds_of(ui: &mut Ui, selector: &'static str) -> anyhow::Result<gpui::Bounds<Pixels>> {
    ui.bounds(selector)
        .ok_or_else(|| anyhow::anyhow!("no element {selector:?}"))
}

#[test]
fn dragging_across_yaml_selects_whole_lines_and_command_c_copies_them() -> anyhow::Result<()> {
    use gpui::{point, px};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for (appearance, name) in [
        (flock_ui::theme::Appearance::Dark, "selection-dark.png"),
        (flock_ui::theme::Appearance::Light, "selection-light.png"),
    ] {
        let mut ui = Ui::with(
            Screen::ResourceInspector,
            flock::Preset::default(),
            appearance,
        )?;
        let lines = bounds_of(&mut ui, "code-lines")?;
        let first = lines.origin + point(px(1.), px(4.));
        let third = lines.origin + point(px(60.), px(50.));
        ui.drag_between(first, third)?;
        ui.press("cmd-c")?;
        assert_eq!(
            ui.clipboard().as_deref(),
            Some("apiVersion: v1\nkind: ConfigMap\nmetadata")
        );
        assert_eq!(ui.state().location().page, Page::ConfigMaps);
        ui.save_png(&dir.join(name))?;
    }
    Ok(())
}

#[test]
fn clicks_select_a_word_or_a_line_and_command_a_selects_the_whole_block() -> anyhow::Result<()> {
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    let lines = bounds_of(&mut ui, "code-lines")?;
    let kind = lines.origin + point(px(10.), px(30.));
    ui.click_at(kind, 2)?;
    ui.press("cmd-c")?;
    assert_eq!(ui.clipboard().as_deref(), Some("kind"));
    ui.click_at(kind, 3)?;
    ui.press("cmd-c")?;
    assert_eq!(ui.clipboard().as_deref(), Some("kind: ConfigMap"));
    ui.click_at(kind, 1)?;
    ui.press("cmd-a cmd-c")?;
    let all = ui.clipboard().unwrap_or_default();
    assert!(all.starts_with("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: "));
    assert!(all.contains("\n    health:\n      path: /healthz\n"));
    Ok(())
}

#[test]
fn a_row_click_still_selects_the_row_and_a_drag_in_a_cell_selects_its_text() -> anyhow::Result<()> {
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::Events)?;
    let row = bounds_of(&mut ui, "events-row-1")?;
    let border = bounds_of(&mut ui, "events-resize-0")?;
    let start = point(border.right() + px(4.), row.center().y);
    ui.drag_between(start, start + point(px(34.), px(0.)))?;
    assert!(ui.bounds("close-event").is_none());
    ui.press("cmd-c")?;
    let copied = ui.clipboard().unwrap_or_default();
    assert_eq!(copied, "deplo");
    ui.click("events-row-0")?;
    assert!(ui.bounds("close-event").is_some());
    let message = bounds_of(&mut ui, "event-message")?;
    let top_left = message.origin + point(px(1.), px(4.));
    let end = point(message.right() - px(1.), message.bottom() - px(4.));
    ui.drag_between(top_left, end)?;
    ui.press("cmd-c")?;
    let selected = ui.clipboard().unwrap_or_default();
    assert!(selected.len() > copied.len(), "{selected:?}");
    assert!(ui.bounds("close-event").is_some());
    Ok(())
}

#[test]
fn log_lines_select_across_lines() -> anyhow::Result<()> {
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("pods-row-1")?;
    let logs = bounds_of(&mut ui, "workload-logs")?;
    let from = logs.origin + point(px(165.), px(20.));
    let to = logs.origin + point(px(300.), px(80.));
    ui.drag_between(from, to)?;
    ui.press("cmd-c")?;
    assert_eq!(
        ui.clipboard().as_deref(),
        Some(
            "GET /healthz 200 0.3ms\nPOST /v1/cart/9312/items 201 18.2ms\nGET /v1/cart/9312 200 6.9ms\nslow query 412ms:"
        )
    );
    Ok(())
}

#[test]
fn text_selected_inside_a_selected_row_is_painted_with_its_own_colour() -> anyhow::Result<()> {
    use gpui::{point, px};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for (appearance, name) in [
        (
            flock_ui::theme::Appearance::Dark,
            "selection-in-row-dark.png",
        ),
        (
            flock_ui::theme::Appearance::Light,
            "selection-in-row-light.png",
        ),
    ] {
        let mut ui = Ui::with(Screen::Events, flock::Preset::default(), appearance)?;
        ui.click("events-row-1")?;
        let row = bounds_of(&mut ui, "events-row-1")?;
        let border = bounds_of(&mut ui, "events-resize-0")?;
        let start = point(border.right() + px(4.), row.center().y);
        ui.drag_between(start, start + point(px(34.), px(0.)))?;
        ui.press("cmd-c")?;
        assert_eq!(ui.clipboard().as_deref(), Some("deplo"));
        let palette = ui.read_app(|cx| *flock_ui::theme::ActiveTheme::palette(cx));
        assert_ne!(palette.textsel_on_sel, palette.sel);
        ui.save_png(&dir.join(name))?;
    }
    Ok(())
}

#[test]
fn sidebar_shortcuts_survive_removing_the_focused_sidebar() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-shift-e")?;
    for _ in 0..3 {
        ui.press("cmd-b")?;
        assert!(!ui.state().sidebar_open);
        ui.press("cmd-b")?;
        assert!(ui.state().sidebar_open);
    }
    ui.press("cmd-shift-e")?;
    drag_edge(&mut ui, -SIDEBAR)?;
    assert!(!ui.state().sidebar_open);
    ui.press("cmd-b")?;
    assert!(ui.state().sidebar_open);
    ui.press("cmd-k escape cmd-b cmd-b")?;
    assert!(ui.state().sidebar_open);
    Ok(())
}

#[test]
fn app_shortcuts_work_with_content_and_inspector_fields_focused() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    ui.click("resources-Deployment-row-0")?;
    ui.press("cmd-b")?;
    assert!(!ui.state().sidebar_open);
    ui.press("cmd-b")?;
    assert!(ui.state().sidebar_open);
    ui.press("cmd-t")?;
    assert_eq!(ui.state().panes().focused().tabs().len(), 2);
    ui.press("cmd-1")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    ui.press("cmd-2 cmd-w")?;
    assert_eq!(ui.state().panes().focused().tabs().len(), 1);
    ui.press("cmd-\\")?;
    assert_eq!(ui.state().panes().panes().len(), 2);
    ui.press("cmd-shift-c escape cmd-0")?;
    assert!(ui.state().location().cluster.is_none());
    ui.press("cmd-,")?;
    ui.click("settings-clusters")?;
    ui.click("default-namespace")?;
    ui.type_text("future-namespace")?;
    ui.press("cmd-b")?;
    assert!(!ui.state().sidebar_open);
    ui.press("cmd-b cmd-p escape cmd-shift-p escape cmd-k escape")?;
    assert!(ui.state().sidebar_open);
    Ok(())
}

#[test]
fn default_namespace_picker_filters_cluster_namespaces_and_accepts_new_names() -> anyhow::Result<()>
{
    for (appearance, name) in [
        (
            flock_ui::theme::Appearance::Dark,
            "namespace-picker-dark.png",
        ),
        (
            flock_ui::theme::Appearance::Light,
            "namespace-picker-light.png",
        ),
    ] {
        let mut ui = Ui::with(Screen::Cluster, flock::Preset::default(), appearance)?;
        let cluster = ui
            .state()
            .location()
            .cluster
            .clone()
            .ok_or_else(|| anyhow::anyhow!("cluster"))?;
        let names = ui
            .state()
            .snapshot
            .details
            .get(&cluster)
            .ok_or_else(|| anyhow::anyhow!("details"))?
            .namespaces
            .clone();
        let namespace = names
            .first()
            .ok_or_else(|| anyhow::anyhow!("namespace"))?
            .name
            .clone();
        ui.press("cmd-,")?;
        ui.click("settings-clusters")?;
        ui.click("default-namespace")?;
        assert!(ui.bounds("default-namespace-option-0").is_some());
        ui.type_text(&namespace)?;
        ui.press("enter")?;
        assert_eq!(
            ui.state()
                .snapshot
                .catalog
                .get(&cluster)
                .and_then(|settings| settings.preferences.default_namespace.as_deref()),
            Some(namespace.as_str())
        );
        ui.click("default-namespace")?;
        ui.type_text("future-namespace")?;
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        ui.save_png(&dir.join(name))?;
        ui.press("enter")?;
        assert_eq!(
            ui.state()
                .snapshot
                .catalog
                .get(&cluster)
                .and_then(|settings| settings.preferences.default_namespace.as_deref()),
            Some("future-namespace")
        );
        ui.click("default-namespace")?;
        ui.type_text("cancelled-namespace")?;
        ui.press("escape")?;
        assert_eq!(
            ui.state()
                .snapshot
                .catalog
                .get(&cluster)
                .and_then(|settings| settings.preferences.default_namespace.as_deref()),
            Some("future-namespace")
        );
    }
    Ok(())
}

#[test]
fn keyboard_selection_uses_the_row_fill_without_an_outline() -> anyhow::Result<()> {
    const SCALE: f32 = 2.;
    const EDGE: u32 = 1;
    const INSIDE: u32 = 8;
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::Events, flock::Preset::default(), appearance)?;
        ui.press("cmd-e home")?;
        let bounds = ui
            .bounds("events-row-0")
            .ok_or_else(|| anyhow::anyhow!("selected event"))?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("focused.png");
        ui.save_png(&path)?;
        let pixels = image::open(path)?.to_rgba8();
        let device = bounds.to_device_pixels(SCALE);
        let raw_x = i32::from(device.origin.x);
        let raw_y = i32::from(device.origin.y);
        let x = u32::try_from(raw_x)?;
        let y = u32::try_from(raw_y)?;
        let sample = |dx: u32, dy: u32| pixels.get_pixel(x + dx, y + dy);
        assert_eq!(sample(INSIDE, EDGE), sample(INSIDE, INSIDE));
        assert_eq!(sample(EDGE, INSIDE), sample(INSIDE, INSIDE));
        ui.press("down enter")?;
        assert!(ui.bounds("close-event").is_some());
        ui.press("cmd-shift-e home down")?;
        let nodes = bounds_of(&mut ui, "nav-nodes")?.to_device_pixels(SCALE);
        let events = bounds_of(&mut ui, "nav-events")?.to_device_pixels(SCALE);
        let path = dir.path().join("sidebar.png");
        ui.save_png(&path)?;
        let pixels = image::open(path)?.to_rgba8();
        let nodes_x = u32::try_from(i32::from(nodes.origin.x))?;
        let nodes_y = u32::try_from(i32::from(nodes.origin.y))?;
        let events_x = u32::try_from(i32::from(events.origin.x))?;
        let events_y = u32::try_from(i32::from(events.origin.y))?;
        let fill = pixels.get_pixel(nodes_x + INSIDE, nodes_y + INSIDE);
        assert_eq!(pixels.get_pixel(nodes_x + INSIDE, nodes_y + EDGE), fill);
        assert_ne!(pixels.get_pixel(events_x + INSIDE, events_y + INSIDE), fill);
    }
    Ok(())
}

#[test]
fn app_shortcuts_work_with_the_terminal_focused_and_after_the_dock_closes() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::PodsShell)?;
    ui.click("pod-open-shell")?;
    ui.press("cmd-b")?;
    assert!(!ui.state().sidebar_open);
    ui.press("cmd-b")?;
    assert!(ui.state().sidebar_open);
    ui.press("cmd-j")?;
    assert_eq!(ui.read(Shell::dock_open), Some(false));
    ui.press("cmd-j")?;
    assert_eq!(ui.read(Shell::dock_open), Some(true));
    ui.press("cmd-k escape cmd-t")?;
    assert_eq!(ui.state().panes().focused().tabs().len(), 2);
    Ok(())
}

#[test]
fn nav_badges_stay_after_leaving_their_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-deployments")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    assert!(ui.bounds("nav-deployments-err").is_some());
    ui.click("nav-overview")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    assert!(ui.bounds("nav-deployments-err").is_some());
    assert!(ui.bounds("nav-deployments-total").is_none());
    Ok(())
}

fn without_detail(cluster: &ClusterId) -> flock_core::Update {
    let mut snapshot = flock_demo::snapshot();
    snapshot.details.remove(cluster);
    snapshot.pages.remove(cluster);
    flock_core::Update::Snapshot(Box::new(snapshot))
}

#[test]
fn returning_to_a_tab_renders_its_data_without_a_skeleton() -> anyhow::Result<()> {
    for (screen, shown) in [
        (Screen::Resources, "resources-Deployment-row-0"),
        (Screen::Cluster, "nav-overview"),
    ] {
        let (mut ui, updates) = Ui::scripted(screen)?;
        let Some(cluster) = ui.state().location().cluster.clone() else {
            anyhow::bail!("{screen:?} opens on a cluster page");
        };
        assert!(ui.bounds(shown).is_some());
        ui.press("cmd-t")?;
        updates.try_send(without_detail(&cluster))?;
        ui.settle()?;
        ui.press("cmd-1")?;
        assert_eq!(ui.state().location().cluster.as_ref(), Some(&cluster));
        assert!(
            ui.bounds("loading-skeleton").is_none(),
            "{screen:?} fell back to the skeleton"
        );
        assert!(ui.bounds(shown).is_some(), "{screen:?} lost {shown}");
    }
    Ok(())
}

fn motion_on(ui: &mut Ui) {
    ui.update_app(|cx| cx.set_reduce_motion(false));
}

fn tab_gap(ui: &mut Ui) -> Option<Pixels> {
    let first = ui.bounds("tab-0")?;
    let second = ui.bounds("tab-1")?;
    Some(second.left() - first.right())
}

#[test]
fn a_new_tab_grows_in_and_ends_beside_its_neighbour() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    motion_on(&mut ui);
    ui.press_unsettled("cmd-t")?;
    assert!(ui.bounds("tab-slot-1").is_some());
    ui.advance_clock(TAB_ENTER)?;
    assert!(ui.bounds("tab-slot-1").is_none());
    assert_eq!(tab_gap(&mut ui), Some(gpui::px(0.)));
    Ok(())
}

fn nodes_with_an_inspector_and_a_second_tab() -> anyhow::Result<Ui> {
    let mut ui = Ui::open(Screen::Nodes)?;
    motion_on(&mut ui);
    ui.press("cmd-t")?;
    ui.click("tab-0")?;
    ui.press("cmd-e down")?;
    ui.advance_clock(TAB_ENTER.max(INSPECTOR_IN))?;
    assert_eq!(tab_gap(&mut ui), Some(gpui::px(0.)));
    Ok(ui)
}

fn at_rest_while_switching_rows(
    ui: &mut Ui,
    pane: &'static str,
    content: &'static str,
) -> anyhow::Result<()> {
    ui.advance_clock(INSPECTOR_IN)?;
    ui.press_unsettled("down")?;
    let switching = (bounds_of(ui, pane)?, bounds_of(ui, content)?);
    ui.advance_clock(INSPECTOR_IN)?;
    let rest = (bounds_of(ui, pane)?, bounds_of(ui, content)?);
    assert_eq!(switching, rest);
    Ok(())
}

#[test]
fn an_open_inspector_swaps_rows_without_replaying_its_entrance() -> anyhow::Result<()> {
    let mut nodes = Ui::open(Screen::Nodes)?;
    motion_on(&mut nodes);
    nodes.click("nodes-row-0")?;
    at_rest_while_switching_rows(&mut nodes, "node-inspector-pane", "node-usage-CPU")?;
    let mut pods = Ui::open(Screen::Pods)?;
    motion_on(&mut pods);
    pods.click("pods-row-0")?;
    at_rest_while_switching_rows(&mut pods, "pod-inspector-pane", "pod-shell")?;
    Ok(())
}

#[test]
fn splitting_keeps_the_existing_tabs_at_rest() -> anyhow::Result<()> {
    let mut ui = nodes_with_an_inspector_and_a_second_tab()?;
    ui.press_unsettled("cmd-\\")?;
    assert_eq!(pane_count(&ui), 2);
    assert_eq!(tab_gap(&mut ui), Some(gpui::px(0.)));
    assert!(ui.bounds("tab-slot-0").is_none());
    assert!(ui.bounds("tab-slot-1").is_none());
    ui.advance_clock(TAB_ENTER)?;
    ui.press_unsettled("cmd-w")?;
    assert_eq!(pane_count(&ui), 1);
    assert!(ui.bounds("tab-slot-0").is_none());
    assert!(ui.bounds("tab-slot-1").is_none());
    Ok(())
}

#[test]
fn splitting_keeps_a_closed_inspector_closed_in_both_panes() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    assert_eq!(ui.state().location().page, Page::ConfigMaps);
    assert!(ui.bounds("resource-inspector-pane").is_some());
    ui.click("close-resource")?;
    assert!(ui.bounds("resource-inspector-pane").is_none());
    ui.press("cmd-\\")?;
    assert_eq!(pane_count(&ui), 2);
    assert!(ui.bounds("resource-inspector-pane").is_none());
    Ok(())
}

#[test]
fn moving_a_tab_to_a_split_keeps_the_open_inspector_at_rest() -> anyhow::Result<()> {
    let mut ui = nodes_with_an_inspector_and_a_second_tab()?;
    ui.right_click("tab-1")?;
    ui.press_unsettled("down enter")?;
    assert_eq!(pane_count(&ui), 2);
    let moving = bounds_of(&mut ui, "node-inspector-pane")?;
    ui.advance_clock(INSPECTOR_IN)?;
    let rest = bounds_of(&mut ui, "node-inspector-pane")?;
    assert_eq!(moving, rest);
    Ok(())
}

#[test]
fn history_buttons_walk_and_disable_at_each_end() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Flock)?;
    let start = ui.state().location().clone();
    assert!(!ui.state().panes().active().can_go_back());
    assert!(!ui.state().panes().active().can_go_forward());
    ui.right_click("pane-0-back")?;
    assert!(ui.bounds("history-menu-item-0").is_none());
    ui.click("pane-0-back")?;
    ui.click("pane-0-forward")?;
    assert_eq!(ui.state().location(), &start);
    ui.click("nav-gitops")?;
    ui.click("nav-changes")?;
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location().page, Page::GitOps);
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location(), &start);
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location(), &start);
    ui.click("pane-0-forward")?;
    assert_eq!(ui.state().location().page, Page::GitOps);
    ui.click("pane-0-forward")?;
    assert_eq!(ui.state().location().page, Page::Changes);
    ui.click("pane-0-forward")?;
    assert_eq!(ui.state().location().page, Page::Changes);
    Ok(())
}

#[test]
fn a_closed_tab_leaves_the_model_at_once_and_shrinks_away() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-t")?;
    motion_on(&mut ui);
    ui.press_unsettled("cmd-w")?;
    assert_eq!(tab_pages(&ui).len(), 1);
    assert!(ui.bounds("tab-leaving-1").is_some());
    ui.advance_clock(TAB_LEAVE)?;
    assert!(ui.bounds("tab-leaving-1").is_none());
    assert!(ui.bounds("tab-1").is_none());
    Ok(())
}

#[test]
fn history_menus_jump_in_both_directions_most_recent_first() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    ui.click("nav-pods")?;
    ui.click("nav-events")?;
    ui.right_click("pane-0-back")?;
    ui.click("history-menu-item-1")?;
    assert_eq!(ui.state().location().page, Page::Nodes);
    ui.right_click("pane-0-forward")?;
    ui.click("history-menu-item-1")?;
    assert_eq!(ui.state().location().page, Page::Events);
    ui.right_click("pane-0-back")?;
    ui.press("down enter")?;
    assert_eq!(ui.state().location().page, Page::Pods);
    Ok(())
}

#[test]
fn reduce_motion_shows_the_final_strip_immediately() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press_unsettled("cmd-t")?;
    assert!(ui.bounds("tab-slot-1").is_none());
    assert_eq!(tab_gap(&mut ui), Some(gpui::px(0.)));
    ui.press_unsettled("cmd-w")?;
    assert!(ui.bounds("tab-leaving-1").is_none());
    assert!(ui.bounds("tab-1").is_none());
    Ok(())
}

#[test]
fn split_history_controls_and_mouse_navigation_follow_the_pointer() -> anyhow::Result<()> {
    use gpui::NavigationDirection::{Back, Forward};
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    ui.press("cmd-\\")?;
    ui.click("nav-pods")?;
    ui.click("pane-0-back")?;
    assert_eq!(
        ui.state().panes().panes()[0].active().location().page,
        Page::ClusterOverview
    );
    assert_eq!(
        ui.state().panes().panes()[1].active().location().page,
        Page::Pods
    );
    ui.click("pane-1-back")?;
    assert_eq!(
        ui.state().panes().panes()[1].active().location().page,
        Page::Nodes
    );
    ui.navigate("pane-0-forward", Forward)?;
    assert_eq!(
        ui.state().panes().panes()[0].active().location().page,
        Page::Nodes
    );
    ui.navigate("pane-1-forward", Forward)?;
    assert_eq!(
        ui.state().panes().panes()[1].active().location().page,
        Page::Pods
    );
    ui.navigate("pane-0-back", Back)?;
    assert_eq!(
        ui.state().panes().panes()[0].active().location().page,
        Page::ClusterOverview
    );
    assert_eq!(
        ui.state().panes().panes()[1].active().location().page,
        Page::Pods
    );
    Ok(())
}

#[test]
fn a_right_clicked_row_menu_takes_the_keyboard() -> anyhow::Result<()> {
    for (screen, row, item) in [
        (Screen::Nodes, "nodes-row-0", "nodes-row-menu-item-0"),
        (Screen::Pods, "pods-row-0", "pods-row-menu-item-0"),
        (
            Screen::Resources,
            "resources-Deployment-row-0",
            "resources-Deployment-row-menu-item-0",
        ),
    ] {
        let mut ui = Ui::open(screen)?;
        ui.right_click(row)?;
        assert!(ui.bounds(item).is_some(), "{screen:?}: no row menu");
        ui.press("escape")?;
        assert!(
            ui.bounds(item).is_none(),
            "{screen:?}: escape kept the menu"
        );
    }
    Ok(())
}

#[test]
fn filter_namespace_completion_chips_and_backspace_work_in_both_appearances() -> anyhow::Result<()>
{
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for (appearance, name) in [
        (flock_ui::theme::Appearance::Dark, "dark"),
        (flock_ui::theme::Appearance::Light, "light"),
    ] {
        let mut ui = Ui::with(Screen::Resources, flock::Preset::default(), appearance)?;
        assert!(ui.bounds("resource-filter-shortcut").is_some());
        ui.save_png(&dir.join(format!("filter-empty-{name}.png")))?;
        ui.press("/")?;
        ui.type_text("ns:")?;
        assert!(ui.bounds("resource-filter-shortcut").is_none());
        assert!(ui.bounds("resource-filter-suggestion-0").is_some());
        ui.save_png(&dir.join(format!("filter-suggestions-{name}.png")))?;
        ui.hover("resource-filter-suggestion-0")?;
        for row in 0..ROWS_UNDER_POPOVER {
            let more = selector(format!("{DEPLOYMENTS}-row-{row}-more"));
            assert!(ui.bounds(more).is_none());
        }
        ui.press("down up tab")?;
        assert!(ui.bounds("resource-filter-token-0").is_some());
        assert!(ui.bounds("resource-filter-suggestion-0").is_none());
        let namespace = ui.read(|shell, cx| {
            shell
                .filter_bar(cx)
                .and_then(|bar| bar.read(cx).query().namespace.clone())
        });
        assert!(namespace.is_some());
        ui.save_png(&dir.join(format!("filter-chips-{name}.png")))?;
        ui.press("backspace")?;
        assert!(ui.bounds("resource-filter-token-0").is_none());
        assert!(ui.read(|shell, cx| {
            shell
                .filter_bar(cx)
                .is_some_and(|bar| bar.read(cx).query().is_empty())
        }));
        ui.type_text("na")?;
        ui.press("enter")?;
        assert!(ui.bounds("resource-filter-suggestion-0").is_some());
        ui.press("escape")?;
        assert!(ui.bounds("resource-filter-suggestion-0").is_none());
        assert!(ui.read(|shell, cx| {
            shell
                .filter_bar(cx)
                .is_some_and(|bar| bar.read(cx).query().namespace.as_deref() == Some(""))
        }));
    }
    Ok(())
}

#[test]
fn the_pod_phase_filter_keeps_one_phase() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    assert!(ui.bounds("pods-row-9").is_some());
    ui.click("pod-filter-chips-2")?;
    assert!(ui.bounds("pods-row-9").is_some());
    ui.click("pod-filter-chips-5")?;
    assert!(ui.bounds("pods-row-0").is_none());
    ui.click("pod-filter-chips-0")?;
    assert!(ui.bounds("pods-row-9").is_some());
    Ok(())
}

#[test]
fn a_namespace_filter_links_to_pods_failing_in_other_namespaces() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    assert!(ui.bounds("pod-failing-elsewhere").is_some());
    let before = picked(&ui, SAMPLE, POD_LIST);
    ui.click("pod-failing-elsewhere")?;
    let after = picked(&ui, SAMPLE, POD_LIST);
    assert_ne!(after, before);
    assert!(after.is_some());
    assert!(ui.bounds("pod-failing-elsewhere").is_none());
    ui.click("pod-namespace")?;
    ui.click("pod-namespace-option-1")?;
    assert_ne!(picked(&ui, SAMPLE, POD_LIST), after);
    Ok(())
}

#[test]
fn the_pod_owner_chain_links_each_owner() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    let row = (0..POD_ROWS).find(|ix| {
        let row: &'static str = format!("pods-row-{ix}").leak();
        ui.click(row).is_ok() && ui.bounds("pod-owner-1").is_some()
    });
    let row = row.context("no demo pod shows a two-step owner chain")?;
    let row: &'static str = format!("pods-row-{row}").leak();
    for (link, kind) in [("pod-owner-0", "Deployment"), ("pod-owner-1", "ReplicaSet")] {
        let mut ui = Ui::open(Screen::Pods)?;
        ui.click(row)?;
        ui.click(link)?;
        assert_eq!(opened(&ui).map(|(kind, _)| kind).as_deref(), Some(kind));
    }
    Ok(())
}

#[test]
fn the_pods_dock_waits_for_a_pod_before_offering_log_controls() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    assert!(ui.bounds("pod-logs-follow").is_some());
    assert!(ui.bounds("pods-dock-empty").is_none());
    ui.press("/")?;
    ui.type_text("nothing-is-called-this")?;
    assert!(ui.bounds("pod-logs-follow").is_none());
    assert!(ui.bounds("pod-logs-download").is_none());
    assert!(ui.bounds("pods-dock-empty").is_some());
    ui.click("empty-clear-filter")?;
    assert!(ui.bounds("pod-logs-follow").is_some());
    Ok(())
}

#[test]
fn history_buttons_follow_the_active_tab_and_clear_forward_on_a_new_visit() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-nodes")?;
    ui.press("cmd-t")?;
    ui.click("nav-gitops")?;
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location().page, Page::Overview);
    ui.click("tab-0")?;
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.click("nav-pods")?;
    assert!(!ui.state().panes().active().can_go_forward());
    ui.click("pane-0-forward")?;
    assert_eq!(ui.state().location().page, Page::Pods);
    ui.click("tab-1")?;
    ui.click("pane-0-forward")?;
    assert_eq!(ui.state().location().page, Page::GitOps);
    Ok(())
}

#[test]
fn a_list_header_has_no_count_while_its_rows_are_loading() -> anyhow::Result<()> {
    use flock_shots::StateFixture;
    let mut loaded = Ui::open(Screen::Resources)?;
    assert!(loaded.bounds("page-note").is_some());
    let mut loading = Ui::with_state(Screen::Resources, StateFixture::Loading)?;
    assert!(loading.bounds("loading-skeleton").is_some());
    assert!(loading.bounds("page-note").is_none());
    Ok(())
}

#[test]
fn the_cluster_overview_title_carries_no_status_badge() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    assert!(ui.bounds("overview-settings").is_some());
    assert!(ui.bounds("page-note").is_none());
    Ok(())
}

#[test]
fn every_utilization_window_keeps_the_chart_and_local_collection_note() -> anyhow::Result<()> {
    let mut ui = Ui::with_state(Screen::Cluster, flock_shots::StateFixture::MetricsLocal)?;
    for selector in ["range-0", "range-1", "range-2", "range-3"] {
        ui.click(selector)?;
        assert!(ui.bounds("utilization-chart").is_some());
        assert!(ui.bounds("metrics-note").is_some(), "{selector}");
        assert!(ui.bounds("metrics-empty").is_none(), "{selector}");
        assert!(ui.bounds("chart-y-axis").is_some(), "{selector}");
        let plot = ui.bounds("chart-plot").context("plot")?;
        for label in [
            "chart-x-label-0",
            "chart-x-label-1",
            "chart-x-label-2",
            "chart-x-label-3",
            "chart-x-label-4",
        ] {
            let at = ui.bounds(label).context(label)?;
            assert!(at.left() >= plot.left(), "{selector} {label}");
            assert!(at.right() <= plot.right(), "{selector} {label}");
        }
    }
    Ok(())
}

#[test]
fn empty_utilization_opens_the_selected_clusters_metrics_settings() -> anyhow::Result<()> {
    let mut ui = Ui::with_state(Screen::Cluster, flock_shots::StateFixture::MetricsEmpty)?;
    ui.click("range-1")?;
    assert!(ui.bounds("utilization-chart").is_some());
    let empty = ui.bounds("metrics-empty").context("empty state")?;
    let area = ui.bounds("chart-message").context("chart message")?;
    assert!((empty.center().x - area.center().x).abs() < gpui::px(CENTRE_TOLERANCE));
    assert!((empty.center().y - area.center().y).abs() < gpui::px(CENTRE_TOLERANCE));
    assert!(ui.bounds("metrics-note").is_none());
    assert!(ui.bounds("chart-plot").is_none());
    assert!(ui.bounds("chart-y-axis").is_none());
    assert!(ui.bounds("chart-x-axis").is_none());
    let origin = ui.state().location().cluster.clone();
    ui.click("metrics-settings")?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    assert!(origin.is_some());
    assert_eq!(settings_selection(&ui), origin);
    Ok(())
}

#[test]
fn prometheus_failure_keeps_the_chart_and_offers_retry() -> anyhow::Result<()> {
    let mut ui = Ui::with_state(Screen::Cluster, flock_shots::StateFixture::MetricsError)?;
    ui.click("range-3")?;
    assert!(ui.bounds("utilization-chart").is_some());
    assert!(ui.bounds("error-state").is_some());
    assert!(ui.bounds("metrics-empty").is_none());
    ui.click("metrics-retry")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    Ok(())
}

#[test]
fn label_completion_keeps_free_text_and_tokens_can_be_removed_by_click() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.press("/")?;
    ui.type_text("checkout l:")?;
    assert!(ui.bounds("pod-filter-suggestion-0").is_some());
    ui.press("enter")?;
    assert!(ui.bounds("pod-filter-suggestion-0").is_some());
    ui.press("enter")?;
    assert!(ui.bounds("pod-filter-token-0").is_some());
    assert_eq!(filter_words(&ui), Some(vec!["checkout".into()]));
    ui.click("pod-filter-remove-0")?;
    assert!(ui.bounds("pod-filter-token-0").is_none());
    assert_eq!(filter_words(&ui), Some(vec!["checkout".into()]));
    ui.press("cmd-a")?;
    ui.type_text("ns:payments ")?;
    assert!(ui.bounds("pod-filter-token-0").is_some());
    ui.press("backspace")?;
    assert!(ui.bounds("pod-filter-token-0").is_none());
    Ok(())
}

#[test]
fn every_list_turns_typed_tokens_into_removable_chips() -> anyhow::Result<()> {
    let lists = [
        (Screen::Nodes, "node-filter"),
        (Screen::Namespaces, "namespace-filter"),
        (Screen::Events, "event-filter"),
        (Screen::Resources, "resource-filter"),
        (Screen::Pods, "pod-filter"),
        (Screen::Gitops, "gitops-filter"),
        (Screen::Changes, "changes-filter"),
        #[cfg(feature = "loco")]
        (Screen::LocoApps, "loco-app-filter"),
        #[cfg(feature = "loco")]
        (Screen::LocoBuilds, "loco-build-filter"),
        #[cfg(feature = "loco")]
        (Screen::LocoRoutes, "loco-route-filter"),
    ];
    for (screen, id) in lists {
        let mut ui = Ui::open(screen)?;
        let token = selector(format!("{id}-token-0"));
        ui.press("/")?;
        ui.type_text("label:team=checkout ")?;
        assert!(ui.bounds(token).is_some(), "{screen:?}: no chip");
        ui.press("backspace")?;
        assert!(ui.bounds(token).is_none(), "{screen:?}: chip kept");
        ui.type_text("unmatched-name")?;
        ui.press("enter")?;
        assert_eq!(
            filter_words(&ui),
            Some(vec!["unmatched-name".into()]),
            "{screen:?}"
        );
    }
    Ok(())
}

fn push_toast(ui: &mut Ui, toast: flock_ui::kit::Toast) {
    ui.update_app(|cx| flock_ui::kit::toast(toast, cx));
}

#[test]
fn errors_persist_while_success_and_warnings_auto_dismiss() -> anyhow::Result<()> {
    use flock_core::Status;
    use flock_ui::kit::{Toast, toast_statuses};
    use flock_ui::theme::{TOAST_SUCCESS_TIMEOUT, TOAST_WARNING_TIMEOUT};
    let mut ui = Ui::open(Screen::Flock)?;
    push_toast(&mut ui, Toast::new(Status::Err, "Reconcile failed"));
    push_toast(&mut ui, Toast::new(Status::Warn, "Drain incomplete"));
    push_toast(&mut ui, Toast::new(Status::Ok, "Scaled"));
    ui.settle()?;
    let all = ui.read_app(toast_statuses);
    assert_eq!(all, [Status::Err, Status::Warn, Status::Ok]);
    ui.advance_clock(TOAST_SUCCESS_TIMEOUT)?;
    let after_success = ui.read_app(toast_statuses);
    assert_eq!(after_success, [Status::Err, Status::Warn]);
    ui.advance_clock(TOAST_WARNING_TIMEOUT.abs_diff(TOAST_SUCCESS_TIMEOUT))?;
    let after_warning = ui.read_app(toast_statuses);
    assert_eq!(after_warning, [Status::Err]);
    ui.advance_clock(TOAST_WARNING_TIMEOUT * 4)?;
    let later = ui.read_app(toast_statuses);
    assert_eq!(later, [Status::Err]);
    ui.click("toast-close")?;
    let remaining = ui.read_app(toast_statuses);
    assert_eq!(remaining, Vec::<Status>::new());
    Ok(())
}

#[test]
fn an_error_toast_shows_its_action_and_running_it_dismisses_the_toast() -> anyhow::Result<()> {
    use flock_core::Status;
    use flock_ui::kit::{Toast, toast_statuses};
    use std::cell::Cell;
    use std::rc::Rc;
    let mut ui = Ui::open(Screen::Flock)?;
    let retried = Rc::new(Cell::new(false));
    let flag = retried.clone();
    let failed = Toast::new(Status::Err, "Could not update Flock")
        .action("Retry", move |_, _| flag.set(true));
    push_toast(&mut ui, failed);
    ui.settle()?;
    assert!(ui.bounds("toast-action-0").is_some());
    ui.click("toast-action-0")?;
    assert!(retried.get());
    let remaining = ui.read_app(toast_statuses);
    assert_eq!(remaining, Vec::<Status>::new());
    Ok(())
}

#[test]
fn a_progress_toast_is_replaced_in_place_by_its_result() -> anyhow::Result<()> {
    use flock_core::Status;
    use flock_ui::kit::{Toast, toast_statuses, toast_titles};
    let mut ui = Ui::open(Screen::Flock)?;
    let key = "reconcile";
    push_toast(
        &mut ui,
        Toast::new(Status::Progress, "Reconciling").key(key),
    );
    push_toast(&mut ui, Toast::new(Status::Ok, "Other"));
    push_toast(&mut ui, Toast::new(Status::Ok, "Reconciled").key(key));
    ui.settle()?;
    let titles = ui.read_app(toast_titles);
    assert_eq!(titles, ["Reconciled", "Other"]);
    let statuses = ui.read_app(toast_statuses);
    assert_eq!(statuses, [Status::Ok, Status::Ok]);
    Ok(())
}

fn pod_logs(ui: &Ui) -> (Vec<String>, String) {
    ui.read(Shell::pod_logs).unwrap_or_default()
}

#[test]
fn a_workloads_logs_tab_merges_every_pod_in_time_order() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Logs)?;
    ui.settle()?;
    let state = ui.state();
    assert_eq!(state.location().page, Page::Logs);
    assert!(state.location().logs.is_some());
    let (legend, text) = pod_logs(&ui);
    assert!(legend.len() >= 2, "{legend:?}");
    let times: Vec<&str> = text
        .lines()
        .filter_map(|line| line.split(' ').next())
        .collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(times, sorted);
    for label in &legend {
        assert!(text.contains(label.as_str()), "{label} has no lines");
    }
    ui.click("tab-pod-logs-source-0")?;
    let (shown, filtered) = pod_logs(&ui);
    assert_eq!(shown.len(), legend.len() - 1);
    let hidden = legend.iter().find(|label| !shown.contains(label));
    assert!(hidden.is_some_and(|label| !filtered.contains(&format!(" {label} "))));
    ui.click("tab-pod-logs-source-0")?;
    assert_eq!(pod_logs(&ui).0, legend);
    Ok(())
}

#[test]
fn pausing_the_merged_logs_holds_new_lines_until_resumed() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Logs)?;
    ui.settle()?;
    ui.click("tab-pod-logs-pause")?;
    let held = pod_logs(&ui).1;
    ui.advance_clock(std::time::Duration::from_secs(2))?;
    assert_eq!(pod_logs(&ui).1, held);
    ui.click("tab-pod-logs-pause")?;
    ui.advance_clock(std::time::Duration::from_secs(1))?;
    assert!(pod_logs(&ui).1.len() > held.len());
    Ok(())
}

#[test]
fn the_workload_menu_and_dock_follow_all_pods() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("workload-actions")?;
    ui.click("workload-actions-item-1")?;
    assert_eq!(
        ui.state()
            .location()
            .logs
            .as_ref()
            .map(flock_core::LogTarget::label),
        Some("checkout-api".to_owned())
    );
    ui.click("tab-0")?;
    assert_eq!(ui.state().location().page, Page::Deployments);
    ui.click("logs-all-pods")?;
    ui.settle()?;
    assert!(ui.bounds("workload-pod-logs-legend").is_some());
    assert!(ui.bounds("workload-pod-logs-source-0").is_some());
    ui.click("logs-open-tab")?;
    let state = ui.state();
    assert_eq!(state.location().page, Page::Logs);
    assert_eq!(
        state
            .location()
            .logs
            .as_ref()
            .map(flock_core::LogTarget::label),
        Some("checkout-api".to_owned())
    );
    Ok(())
}

#[test]
fn shift_clicked_pods_open_one_merged_logs_tab() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.click("pods-logs-selected")?;
    assert_eq!(ui.state().location().page, Page::Pods);
    ui.click("pods-row-0")?;
    ui.shift_click("pods-row-1")?;
    ui.click("pods-logs-selected")?;
    let state = ui.state();
    assert_eq!(state.location().page, Page::Logs);
    let target = state.location().logs.clone();
    assert!(
        matches!(&target, Some(flock_core::LogTarget::Pods { pods, .. }) if pods.len() == 2),
        "{target:?}"
    );
    Ok(())
}

#[test]
fn a_label_filter_on_pods_offers_logs_for_the_selector() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.press("/")?;
    ui.type_text("l:app=checkout-api ")?;
    ui.click("pods-logs-selected")?;
    let target = ui.state().location().logs.clone();
    assert_eq!(
        target.as_ref().map(flock_core::LogTarget::label),
        Some("app=checkout-api".to_owned())
    );
    Ok(())
}

#[test]
fn the_cluster_overview_opens_settings_for_its_own_cluster() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("cluster-switcher")?;
    let staging = switcher_row(&ui, "staging")?;
    ui.click(staging)?;
    let staging = Some(ClusterId::new("staging"));
    assert_eq!(ui.state().location().cluster, staging);
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.click("overview-settings")?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    assert_eq!(settings_selection(&ui), staging);
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, Page::ClusterOverview);
    ui.click("overview-actions")?;
    ui.click("overview-actions-item-0")?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    assert_eq!(settings_selection(&ui), staging);
    Ok(())
}

#[test]
fn the_metrics_source_is_picked_from_a_list_that_stays_inside_the_form() -> anyhow::Result<()> {
    use flock_core::settings::MetricsSource;
    use flock_ui::theme::{SETTINGS_FIELD, SETTINGS_INSET};
    let mut ui = Ui::open(Screen::ClusterSettings)?;
    let cluster = settings_selection(&ui).ok_or_else(|| anyhow::anyhow!("no cluster"))?;
    let window = bounds_of(&mut ui, "tab-strip")?;
    for selector in ["metrics-source", "default-namespace"] {
        let field = bounds_of(&mut ui, selector)?;
        assert!(field.size.width <= SETTINGS_FIELD, "{selector}");
        assert!(
            field.right() <= window.right() - SETTINGS_INSET,
            "{selector}"
        );
    }
    ui.click("metrics-source")?;
    ui.click("metrics-source-option-2")?;
    let metrics = ui
        .state()
        .snapshot
        .catalog
        .get(&cluster)
        .map(|s| s.preferences.metrics);
    assert_eq!(metrics, Some(MetricsSource::Prometheus));
    Ok(())
}

#[test]
fn the_sidebar_header_and_page_header_read_as_one_row_over_one_content_edge() -> anyhow::Result<()>
{
    use flock_ui::theme::{ICON_BUTTON_INSET, SPACE_8};
    let mut ui = Ui::open(Screen::Nodes)?;
    let scope = bounds_of(&mut ui, "sidebar-scope")?;
    let header = bounds_of(&mut ui, "page-header")?;
    assert_eq!(scope.size.height, header.size.height);
    assert_eq!(scope.center().y, header.center().y);
    let back = bounds_of(&mut ui, "pane-0-back")?;
    let filter = bounds_of(&mut ui, "node-filter")?;
    assert_eq!(back.left() + ICON_BUTTON_INSET, filter.left() + SPACE_8);
    Ok(())
}

#[test]
fn editing_yaml_validates_reviews_a_dry_run_and_applies_after_confirming() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::EditYaml)?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    assert!(ui.bounds("edit-footer").is_some());
    ui.click("edit-editor")?;
    ui.press("cmd-a")?;
    ui.type_text("kind: [")?;
    assert!(ui.bounds("edit-invalid").is_some());
    ui.press("cmd-enter")?;
    assert!(ui.bounds("edit-review-view").is_none());
    ui.press("cmd-a")?;
    ui.type_text("{apiVersion: v1, kind: ConfigMap, metadata: {name: checkout-api-config, namespace: payments}, data: {level: debug}}")?;
    assert!(ui.bounds("edit-invalid").is_none());
    ui.press("cmd-enter")?;
    assert!(ui.bounds("edit-review-view").is_some());
    ui.press("cmd-enter")?;
    assert!(ui.bounds("dialog-cancel").is_some());
    ui.click("dialog-confirm")?;
    assert!(ui.bounds("edit-editor").is_some());
    let notices = ui.read_app(flock_ui::kit::toast_titles);
    assert!(
        notices
            .iter()
            .any(|title| title.as_ref() == "Applied ConfigMap/checkout-api-config"),
        "{notices:?}"
    );
    Ok(())
}

#[test]
fn a_row_menu_opens_the_object_in_an_editor_tab() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Resources)?;
    let tabs = ui.state().panes().focused().tabs().len();
    ui.right_click(selector(format!("{DEPLOYMENTS}-row-1")))?;
    ui.click(selector(format!("{DEPLOYMENTS}-row-menu-item-1")))?;
    let state = ui.state();
    assert_eq!(state.location().page, Page::EditYaml);
    assert_eq!(state.panes().focused().tabs().len(), tabs + 1);
    Ok(())
}

#[test]
#[ignore = "renders the YAML editor states for review"]
fn edit_yaml_states_render_for_review() -> anyhow::Result<()> {
    for (appearance, name) in [
        (flock_ui::theme::Appearance::Dark, "dark"),
        (flock_ui::theme::Appearance::Light, "light"),
    ] {
        let mut ui = Ui::with(Screen::EditYaml, flock::Preset::default(), appearance)?;
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        ui.click("edit-editor")?;
        ui.press("cmd-up")?;
        for _ in 0..10 {
            ui.press("down")?;
        }
        ui.press("cmd-right")?;
        for _ in 0..4 {
            ui.press("backspace")?;
        }
        ui.type_text("9090")?;
        ui.save_png(&dir.join(format!("edit-yaml-typed-{name}.png")))?;
        ui.press("cmd-enter")?;
        ui.save_png(&dir.join(format!("edit-yaml-review-{name}.png")))?;
        ui.press("cmd-enter")?;
        ui.save_png(&dir.join(format!("edit-yaml-confirm-{name}.png")))?;
    }
    Ok(())
}

fn top(ui: &mut Ui, selector: &'static str) -> anyhow::Result<Pixels> {
    ui.bounds(selector)
        .map(|b| b.origin.y)
        .ok_or_else(|| anyhow::anyhow!("no {selector}"))
}

#[test]
fn unhealthy_job_inspector_leads_with_its_error_block_then_conditions_events_and_yaml()
-> anyhow::Result<()> {
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::JobInspector, flock::Preset::default(), appearance)?;
        assert!(ui.bounds("reason-cell").is_some());
        let wrong = top(&mut ui, "error-block-0")?;
        let conditions = top(&mut ui, "condition-table")?;
        let events = top(&mut ui, "object-events")?;
        let yaml = top(&mut ui, "code-horizontal")?;
        assert!(wrong < conditions && conditions < events && events < yaml);
        let pane = ui
            .bounds("resource-inspector-pane")
            .ok_or_else(|| anyhow::anyhow!("no inspector"))?;
        let block = ui
            .bounds("error-block-0")
            .ok_or_else(|| anyhow::anyhow!("no summary"))?;
        assert!(block.origin.x + block.size.width <= pane.origin.x + pane.size.width);
    }
    Ok(())
}

const CONFIG_ROWS: usize = 40;

#[test]
fn back_from_a_related_link_reopens_the_inspector_it_left() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    let inspected = ui.state().location().inspecting.clone();
    assert!(inspected.is_some());
    ui.click("close-resource")?;
    assert!(ui.bounds("resource-inspector-pane").is_none());
    assert_eq!(ui.state().location().inspecting, None);
    let row = (0..CONFIG_ROWS).find(|ix| {
        let clicked = ui.click(selector(format!("resources-ConfigMap-row-{ix}")));
        clicked.is_ok() && ui.state().location().inspecting == inspected
    });
    assert!(row.is_some());
    assert!(!ui.state().panes().active().can_go_forward());
    ui.click("related-Deployments-checkout-api")?;
    let deployment = ui.state().location().clone();
    assert_eq!(
        deployment.workload().map(|target| target.name.as_str()),
        Some("checkout-api")
    );
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location().page, Page::ConfigMaps);
    assert_eq!(ui.state().location().inspecting, inspected);
    assert!(ui.bounds("resource-inspector-pane").is_some());
    assert!(ui.bounds("related-Deployments-checkout-api").is_some());
    ui.press("cmd-]")?;
    assert_eq!(ui.state().location(), &deployment);
    ui.press("cmd-[")?;
    assert!(ui.bounds("resource-inspector-pane").is_some());
    Ok(())
}

#[test]
fn a_namespace_page_opens_what_needs_attention_contents_and_events() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Namespace)?;
    let namespace = ui.state().location().clone();
    assert_eq!(
        namespace.detail().map(|target| target.name.as_str()),
        Some("payments")
    );
    let first = ui
        .state()
        .snapshot
        .home
        .problems
        .iter()
        .find(|problem| problem.target.namespace.as_deref() == Some("payments"))
        .map(|problem| problem.target.name.clone());
    assert!(ui.bounds("namespace-issue-row-0").is_some());
    assert!(ui.bounds("error-blocks").is_none());
    assert!(ui.bounds("namespace-inspector-labels-body").is_none());
    ui.click("namespace-inspector-labels-toggle")?;
    assert!(ui.bounds("namespace-inspector-labels-body").is_some());
    let services = ui
        .state()
        .snapshot
        .detail(
            namespace
                .cluster
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no cluster"))?,
        )
        .map(|detail| detail.overview.unready_services.len());
    assert_eq!(services, Some(1));
    ui.click("namespace-issue-row-0")?;
    assert_eq!(
        ui.state().workload().map(|target| target.name.clone()),
        first
    );
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location(), &namespace);
    ui.click("namespace-Deployments-row-0")?;
    let workload = ui.state().workload().map(|target| target.name.clone());
    assert_eq!(workload.as_deref(), Some("checkout-api"));
    ui.click("pane-0-back")?;
    assert!(ui.bounds("namespace-Pods-row-0").is_none());
    ui.click("namespace-group-Pods")?;
    ui.click("namespace-Pods-row-0")?;
    let pod = ui.state().location().clone();
    assert_eq!(pod.page, Page::Pods);
    assert_eq!(
        pod.resource.map(|target| target.name),
        Some("checkout-api-7f9c6d-x2lqp".to_owned())
    );
    ui.click("pane-0-back")?;
    assert!(ui.bounds("close-event").is_none());
    ui.click("namespace-event-0")?;
    assert_eq!(ui.state().location().page, Page::Events);
    assert!(ui.bounds("close-event").is_some());
    assert!(ui.bounds("event-message").is_some());
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location(), &namespace);
    ui.click("namespace-all-events")?;
    assert_eq!(ui.state().location().page, Page::Events);
    let scoped = ui
        .state()
        .snapshot
        .catalog
        .get(&ClusterId::new("prod-us-east"))
        .map(|settings| settings.preferences.namespace_selection("Event"));
    assert_eq!(
        scoped.as_ref().and_then(|selection| selection.namespace()),
        Some("payments")
    );
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location(), &namespace);
    Ok(())
}

#[test]
fn namespace_tile_problem_counts_open_their_list_on_the_rows_that_need_attention()
-> anyhow::Result<()> {
    let payments = Some(flock_core::settings::NamespaceSelection::Named(
        "payments".into(),
    ));
    let drills = [
        ("namespace-tile-pods-1-failing", Page::Pods, "Pod"),
        (
            "namespace-tile-deployments-1-unavailable",
            Page::Deployments,
            "Deployment",
        ),
        (
            "namespace-tile-services-1-endpoints-not-ready",
            Page::Services,
            "Service",
        ),
    ];
    for (selector, page, list) in drills {
        let mut ui = Ui::open(Screen::Namespace)?;
        ui.click(selector)?;
        assert_eq!(needs_attention_in(&ui, page, list), payments, "{selector}");
    }
    let mut ui = Ui::open(Screen::Namespace)?;
    ui.click("namespace-tile-statefulsets")?;
    assert_eq!(ui.state().location().page, Page::StatefulSets);
    assert_eq!(picked(&ui, "prod-us-east", "StatefulSet"), payments);
    Ok(())
}

#[test]
fn a_namespace_page_scopes_the_list_it_opens_before_the_backend_answers() -> anyhow::Result<()> {
    let (mut ui, updates) = Ui::scripted(Screen::Namespace)?;
    let target = ui
        .state()
        .location()
        .detail()
        .cloned()
        .context("the namespace page has a target")?;
    let mut snapshot = flock_demo::snapshot();
    let detail = snapshot
        .details
        .get_mut(&target.cluster)
        .context("the demo cluster has a detail")?;
    let contents = flock_core::related::Group {
        title: "Deployments".into(),
        items: Vec::new(),
        total: 0,
        counts: flock_core::Counts::default(),
        list: None,
    };
    detail.related.insert(
        0,
        flock_core::related::Related {
            target,
            groups: vec![contents],
        },
    );
    updates.try_send(flock_core::Update::Snapshot(Box::new(snapshot)))?;
    ui.settle()?;
    ui.click("namespace-all-events")?;
    assert_eq!(ui.state().location().page, Page::Events);
    let scoped = picked(&ui, "prod-us-east", "Event");
    assert_eq!(
        scoped.as_ref().and_then(|selection| selection.namespace()),
        Some("payments")
    );
    Ok(())
}

#[test]
fn a_detail_page_scopes_the_list_it_opens_before_the_backend_answers() -> anyhow::Result<()> {
    let (mut ui, updates) = Ui::scripted(Screen::Job)?;
    let target = ui
        .state()
        .location()
        .detail()
        .cloned()
        .context("the job page has a target")?;
    let namespace = target.namespace.clone().context("the job is namespaced")?;
    let mut snapshot = flock_demo::snapshot();
    let detail = snapshot
        .details
        .get_mut(&target.cluster)
        .context("the demo cluster has a detail")?;
    let pods = flock_core::related::Group {
        title: "Pods".into(),
        items: Vec::new(),
        total: 0,
        counts: flock_core::Counts::default(),
        list: Some(flock_core::related::ListLink {
            page: Page::Pods,
            namespace: namespace.clone(),
        }),
    };
    detail.related.insert(
        0,
        flock_core::related::Related {
            target,
            groups: vec![pods],
        },
    );
    updates.try_send(flock_core::Update::Snapshot(Box::new(snapshot)))?;
    ui.settle()?;
    ui.click("related-list-Pods")?;
    assert_eq!(ui.state().location().page, Page::Pods);
    assert_eq!(
        picked(&ui, SAMPLE, POD_LIST),
        Some(flock_core::settings::NamespaceSelection::Named(namespace))
    );
    Ok(())
}

#[test]
fn a_namespace_pick_survives_stale_snapshots_and_navigation_before_the_backend_answers()
-> anyhow::Result<()> {
    const BEYOND_ONE_NAMESPACE: &str = "resources-Deployment-row-8";
    let everywhere = Some(flock_core::settings::NamespaceSelection::All);
    let (mut ui, updates) = Ui::scripted(Screen::Resources)?;
    ui.click("resource-namespace")?;
    ui.click("resource-namespace-option-0")?;
    assert_eq!(picked(&ui, SAMPLE, "Deployment"), everywhere);
    let stale = flock_core::Update::Snapshot(Box::new(flock_demo::snapshot()));
    updates.try_send(stale)?;
    ui.settle()?;
    assert_eq!(picked(&ui, SAMPLE, "Deployment"), everywhere);
    assert!(ui.bounds(BEYOND_ONE_NAMESPACE).is_some());
    open_deployments(&mut ui, "staging")?;
    open_deployments(&mut ui, SAMPLE)?;
    assert!(ui.bounds(BEYOND_ONE_NAMESPACE).is_some());
    Ok(())
}

#[test]
fn healthy_objects_have_no_reason_or_error_block() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    assert!(ui.bounds("resource-inspector-pane").is_some());
    assert!(ui.bounds("error-block-0").is_none());
    assert!(ui.bounds("error-blocks").is_none());
    assert!(ui.bounds("reason-cell").is_none());
    assert!(ui.bounds("object-events").is_none());
    assert!(ui.bounds("events-empty").is_some());
    Ok(())
}

#[test]
fn inspector_sections_collapse_and_expand_without_moving_the_sections_above() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Pods)?;
    assert!(ui.bounds("pod-inspector-labels-body").is_some());
    assert!(ui.bounds("pod-inspector-events-body").is_none());
    let details = top(&mut ui, "pod-inspector-details")?;
    let labels = top(&mut ui, "pod-inspector-labels")?;
    let containers = top(&mut ui, "pod-inspector-containers")?;
    ui.click("pod-inspector-labels-toggle")?;
    assert!(ui.bounds("pod-inspector-labels-body").is_none());
    assert_eq!(top(&mut ui, "pod-inspector-details")?, details);
    assert_eq!(top(&mut ui, "pod-inspector-labels")?, labels);
    assert!(top(&mut ui, "pod-inspector-containers")? < containers);
    ui.click("pod-inspector-labels-toggle")?;
    assert!(ui.bounds("pod-inspector-labels-body").is_some());
    assert_eq!(top(&mut ui, "pod-inspector-containers")?, containers);
    Ok(())
}

#[test]
fn the_workload_events_tab_lists_events_as_rows_with_the_message_underneath() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Workload)?;
    ui.click("dock-tab-1")?;
    assert!(ui.bounds("workload-events").is_some());
    let list = bounds_of(&mut ui, "object-events")?;
    let pane = bounds_of(&mut ui, "workload-events")?;
    assert!(list.origin.y >= pane.origin.y);
    Ok(())
}

#[test]
fn workload_pages_show_error_blocks_in_the_panel_not_under_the_name() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Workload)?;
    let block = bounds_of(&mut ui, "error-block-0")?;
    let blocks = bounds_of(&mut ui, "error-blocks")?;
    assert!(blocks.origin.x <= block.origin.x);
    Ok(())
}

#[test]
fn flux_pages_show_a_condition_line_of_one_height_and_no_error_block() -> anyhow::Result<()> {
    let mut heights = Vec::new();
    for screen in [Screen::Kustomization, Screen::Helmrelease, Screen::Source] {
        let mut ui = Ui::open(screen)?;
        heights.push(bounds_of(&mut ui, "flux-condition")?.size.height);
        assert!(ui.bounds("error-blocks").is_none());
    }
    assert!(heights.windows(2).all(|pair| pair[0] == pair[1]));
    Ok(())
}

#[test]
fn clicking_a_namespace_row_shows_its_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Namespaces)?;
    ui.click("namespaces-row-0")?;
    let opened = ui
        .state()
        .location()
        .detail()
        .map(|target| target.kind.clone());
    assert_eq!(opened.as_deref(), Some("Namespace"));
    Ok(())
}

#[test]
fn arrow_keys_move_through_linked_lists_and_enter_opens_the_row() -> anyhow::Result<()> {
    for (screen, page) in [
        (Screen::Namespaces, Page::Namespaces),
        (Screen::Kustomizations, Page::Kustomizations),
        (Screen::HelmReleases, Page::Releases),
    ] {
        let mut ui = Ui::open(screen)?;
        ui.press("cmd-e")?;
        ui.press("down")?;
        ui.press("down")?;
        let location = ui.state().location().clone();
        assert_eq!(
            location.page, page,
            "{screen:?} left the list on an arrow key"
        );
        assert!(location.detail().is_none() && location.flux_object.is_none());
        assert!(location.helm.is_none());
        ui.press("enter")?;
        let opened = ui.state().location().clone();
        let detail = opened.detail().is_some();
        let flux = opened.flux_object.is_some();
        let helm = opened.helm.is_some();
        assert!(detail || flux || helm, "{screen:?} did not open on Enter");
    }
    Ok(())
}

#[test]
fn clicking_a_flux_inventory_row_opens_the_resource() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Kustomization)?;
    ui.click("flux-inventory-row-0")?;
    let state = ui.state();
    let resource = state.location().detail().is_some();
    let workload = state.workload().is_some();
    assert!(resource || workload, "the inventory row did not open");
    Ok(())
}

#[test]
fn the_resource_inspector_links_the_workloads_that_consume_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::ResourceInspector)?;
    assert!(ui.bounds("resource-inspector-pane").is_some());
    ui.click("related-Deployments-checkout-api")?;
    let workload = ui.state().workload().map(|target| target.name.clone());
    assert_eq!(workload.as_deref(), Some("checkout-api"));
    Ok(())
}

#[test]
fn inspector_width_defaults_per_kind_and_a_dragged_width_wins() -> anyhow::Result<()> {
    use flock_ui::theme::{INSPECTOR_RESOURCE, INSPECTOR_WORKLOAD};
    use gpui::{point, px};
    let width = |ui: &mut Ui| ui.bounds("resource-inspector-pane").map(|b| b.size.width);
    let mut job = Ui::open(Screen::JobInspector)?;
    assert_eq!(width(&mut job), Some(INSPECTOR_WORKLOAD));
    let mut config = Ui::open(Screen::ResourceInspector)?;
    assert_eq!(width(&mut config), Some(INSPECTOR_RESOURCE));
    job.drag("resource-inspector-resize", point(px(40.), px(0.)))?;
    assert_eq!(width(&mut job), Some(INSPECTOR_WORKLOAD - px(40.)));
    job.click("close-resource")?;
    job.click("resources-Job-row-2")?;
    assert_eq!(width(&mut job), Some(INSPECTOR_WORKLOAD - px(40.)));
    Ok(())
}

#[test]
fn a_node_inspector_links_its_biggest_pods_by_request() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Node)?;
    assert!(ui.bounds("node-inspector-pane").is_some());
    ui.scroll(
        "node-inspector-pane",
        gpui::point(gpui::px(0.), gpui::px(-1200.)),
    )?;
    ui.click("node-pod-0")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::Pods);
    assert_eq!(
        location.resource.map(|target| target.name),
        Some("search-indexer-0".to_owned())
    );
    ui.click("pane-0-back")?;
    assert_eq!(ui.state().location().page, Page::Nodes);
    Ok(())
}

#[test]
fn a_node_inspector_opens_the_pods_list_filtered_to_its_node() -> anyhow::Result<()> {
    const NODE: &str = "ip-10-4-12-87";
    let mut ui = Ui::open(Screen::Node)?;
    ui.scroll(
        "node-inspector-pane",
        gpui::point(gpui::px(0.), gpui::px(-1200.)),
    )?;
    ui.click("node-inspector-all-pods")?;
    let state = ui.state();
    let location = state.location().clone();
    assert_eq!(location.page, Page::Pods);
    let in_cluster: Vec<_> = state
        .snapshot
        .pods
        .iter()
        .filter(|pod| Some(&pod.cluster) == location.cluster.as_ref())
        .collect();
    let on_node = in_cluster
        .iter()
        .filter(|pod| pod.node.as_deref() == Some(NODE))
        .count();
    assert!(on_node > 0 && on_node < in_cluster.len());
    let text = ui.read(|shell, cx| shell.filter_bar(cx).map(|bar| bar.read(cx).text(cx)));
    assert_eq!(text, Some(format!("node:{NODE}")));
    assert!(ui.bounds("pod-filter-token-0").is_some());
    let shown =
        |ui: &Ui| ui.read(|shell, cx| shell.filter_bar(cx).and_then(|bar| bar.read(cx).matches()));
    assert_eq!(shown(&ui), Some((on_node, in_cluster.len())));
    ui.click("pod-filter-remove-0")?;
    assert_eq!(shown(&ui), Some((in_cluster.len(), in_cluster.len())));
    Ok(())
}

#[test]
fn crashing_pod_inspector_leads_with_its_error_block_above_conditions_and_events()
-> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.press("cmd-j")?;
    ui.scroll(
        "pod-inspector-pane",
        gpui::point(gpui::px(0.), gpui::px(-INSPECTOR_SCROLL)),
    )?;
    assert!(ui.bounds("condition-table").is_none());
    assert!(ui.bounds("object-events").is_none());
    let wrong = top(&mut ui, "error-block-0")?;
    let conditions = top(&mut ui, "pod-inspector-conditions-toggle")?;
    let events = top(&mut ui, "pod-inspector-events-toggle")?;
    assert!(wrong < conditions && conditions < events);
    ui.click("pod-inspector-events-toggle")?;
    assert!(ui.bounds("object-events").is_some());
    ui.click("pod-inspector-conditions-toggle")?;
    assert!(ui.bounds("condition-table").is_some());
    let pane = ui
        .bounds("pod-inspector-pane")
        .ok_or_else(|| anyhow::anyhow!("no inspector"))?;
    assert_eq!(pane.size.width, flock_ui::theme::INSPECTOR_POD);
    Ok(())
}

#[test]
fn the_pod_picked_when_the_list_opens_loads_its_related_objects() -> anyhow::Result<()> {
    let ui = Ui::open(Screen::Pods)?;
    let target = flock_core::ResourceRef {
        cluster: ClusterId::new(SAMPLE),
        kind: "Pod".into(),
        namespace: Some("payments".into()),
        name: FIRST_POD.into(),
    };
    let state = ui.state();
    let related = state
        .snapshot
        .details
        .get(&target.cluster)
        .and_then(|detail| detail.related(&target));
    assert!(related.is_some(), "Related never loaded for {target:?}");
    Ok(())
}

#[test]
fn a_loading_related_section_reserves_one_row() -> anyhow::Result<()> {
    let (mut ui, _updates) = Ui::scripted(Screen::Pods)?;
    ui.press("cmd-j")?;
    ui.scroll(
        "pod-inspector-pane",
        gpui::point(gpui::px(0.), gpui::px(INSPECTOR_SCROLL_TO_END)),
    )?;
    assert!(ui.bounds("loading-skeleton").is_none());
    let loading = ui
        .bounds("related-loading")
        .context("Related shows no loading row")?;
    assert_eq!(loading.size.height, flock_ui::theme::ROW_DOCUMENT);
    Ok(())
}

#[test]
fn picking_another_pod_starts_its_inspector_at_the_top() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    ui.press("cmd-j")?;
    let header = ui.bounds("pod-inspector-header").context("no pod header")?;
    ui.scroll(
        "pod-inspector-pane",
        gpui::point(gpui::px(0.), gpui::px(-INSPECTOR_SCROLL)),
    )?;
    assert!(top(&mut ui, "error-block-0")? < header.bottom());
    ui.click("pods-row-1")?;
    ui.click("pods-row-0")?;
    assert!(top(&mut ui, "error-block-0")? >= header.bottom());
    assert!(top(&mut ui, "pod-inspector-details")? >= header.bottom());
    Ok(())
}

fn inline_header(ui: &mut Ui, panel: &str) -> anyhow::Result<Pixels> {
    let bounds = |ui: &mut Ui, part: &str| {
        let selector: &'static str = format!("{panel}-header{part}").leak();
        ui.bounds(selector)
            .with_context(|| format!("no {selector}"))
    };
    let header = bounds(ui, "")?;
    let title = bounds(ui, "-title")?;
    let status = bounds(ui, "-status")?;
    assert!(
        status.left() >= title.right(),
        "{panel}: the status is not right of the name"
    );
    assert!(
        status.top() < title.bottom() && title.top() < status.bottom(),
        "{panel}: the status is not on the name's row"
    );
    assert!(status.right() <= header.right());
    Ok(header.size.height)
}

#[test]
fn panel_headers_keep_the_status_beside_the_name_at_one_height() -> anyhow::Result<()> {
    let mut pods = Ui::open(Screen::Pods)?;
    let crashing = inline_header(&mut pods, "pod-inspector")?;
    let shell = pods.bounds("pod-shell").context("no Shell")?;
    let status = pods
        .bounds("pod-inspector-header-status")
        .context("no pod status")?;
    assert!(shell.left() >= status.right());
    pods.click("pods-row-1")?;
    let running = inline_header(&mut pods, "pod-inspector")?;
    assert_eq!(crashing, flock_ui::theme::PANEL_HEADER);
    assert_eq!(running, crashing);
    let mut nodes = Ui::open(Screen::Nodes)?;
    nodes.click("nodes-row-0")?;
    let ready = inline_header(&mut nodes, "node-inspector")?;
    nodes.click("nodes-row-9")?;
    let cordoned = inline_header(&mut nodes, "node-inspector")?;
    assert_eq!(ready, flock_ui::theme::PANEL_HEADER);
    assert_eq!(cordoned, ready);
    Ok(())
}

#[test]
fn cordoned_node_shows_its_reason_in_the_inspector() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Nodes)?;
    ui.click("nodes-row-9")?;
    assert!(ui.bounds("error-block-0").is_some());
    ui.click("nodes-row-0")?;
    assert!(ui.bounds("error-block-0").is_none());
    Ok(())
}

#[test]
fn failing_flux_list_and_page_name_the_reason() -> anyhow::Result<()> {
    let mut list = Ui::open(Screen::Kustomizations)?;
    assert!(list.bounds("reason-cell").is_some());
    let mut page = Ui::open(Screen::Kustomization)?;
    let wrong = top(&mut page, "flux-condition")?;
    let conditions = top(&mut page, "condition-table")?;
    assert!(wrong < conditions);
    Ok(())
}

#[test]
fn a_job_page_follows_the_logs_of_all_its_pods() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Job)?;
    let job = ui.state().location().detail().cloned();
    assert_eq!(job.as_ref().map(|target| target.kind.as_str()), Some("Job"));
    ui.click("detail-logs")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::Logs);
    assert!(location.logs.is_some());
    Ok(())
}

#[test]
fn the_pod_namespace_picker_lists_every_cluster_namespace() -> anyhow::Result<()> {
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::Pods, flock::Preset::default(), appearance)?;
        let state = ui.state();
        let cluster = state
            .location()
            .cluster
            .clone()
            .ok_or_else(|| anyhow::anyhow!("cluster"))?;
        let mut names: Vec<String> = state
            .snapshot
            .details
            .get(&cluster)
            .ok_or_else(|| anyhow::anyhow!("details"))?
            .namespaces
            .iter()
            .map(|ns| ns.name.clone())
            .collect();
        let with_pods: std::collections::BTreeSet<&str> = state
            .snapshot
            .pods
            .iter()
            .filter(|pod| pod.cluster == cluster)
            .map(|pod| pod.namespace.as_str())
            .collect();
        let empty = names
            .iter()
            .find(|name| !with_pods.contains(name.as_str()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the demo cluster has a namespace without pods"))?;
        names.extend(with_pods.iter().map(|ns| (*ns).to_owned()));
        names.sort();
        names.dedup();
        ui.click("pod-namespace")?;
        let last = selector(format!("pod-namespace-option-{}", names.len()));
        let past = selector(format!("pod-namespace-option-{}", names.len() + 1));
        assert!(ui.bounds(last).is_some());
        assert!(ui.bounds(past).is_none());
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        ui.save_png(&dir.join(format!("pod-namespace-picker-{appearance:?}.png")))?;
        let ix = names
            .iter()
            .position(|name| *name == empty)
            .ok_or_else(|| anyhow::anyhow!("{empty} is listed"))?;
        ui.click(selector(format!("pod-namespace-option-{}", ix + 1)))?;
        let chosen = ui
            .state()
            .snapshot
            .catalog
            .get(&cluster)
            .map(|settings| settings.preferences.namespace_selection(POD_LIST));
        assert_eq!(
            chosen.as_ref().and_then(|scope| scope.namespace()),
            Some(empty.as_str())
        );
        assert!(ui.bounds("empty-state-namespace").is_some());
    }
    Ok(())
}

fn opened_flux(ui: &Ui) -> Option<(flock_core::FluxKind, String)> {
    let location = ui.state().location().clone();
    location.flux_object.map(|key| (key.kind, key.name))
}

#[test]
fn clicking_a_graph_node_opens_that_flux_object() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::FluxGraph)?;
    assert_eq!(ui.state().location().page, Page::FluxGraph);
    ui.click("flux-graph-node-0")?;
    assert_eq!(
        opened_flux(&ui),
        Some((flock_core::FluxKind::Kustomization, "apps-ml".into()))
    );
    Ok(())
}

#[test]
fn graph_selection_compares_across_clusters_and_arrows_walk_the_edges() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::FluxGraph)?;
    assert!(ui.bounds("flux-graph-compare").is_none());
    ui.right_click("flux-graph-node-0")?;
    assert!(ui.bounds("flux-graph-compare").is_some());
    assert_eq!(opened_flux(&ui), None);
    ui.press("left")?;
    ui.press("enter")?;
    assert_eq!(
        opened_flux(&ui),
        Some((flock_core::FluxKind::GitRepository, "fleet-infra".into()))
    );
    Ok(())
}

#[test]
fn escape_clears_the_graph_selection() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::FluxGraph)?;
    ui.right_click("flux-graph-node-0")?;
    ui.press("escape")?;
    assert!(ui.bounds("flux-graph-compare").is_none());
    Ok(())
}

#[test]
fn flux_events_filter_by_cluster_and_open_their_object() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::FluxEvents)?;
    assert_eq!(ui.state().location().page, Page::FluxEvents);
    assert!(ui.bounds("flux-events-row-8").is_some());
    ui.press("/")?;
    ui.type_text("staging")?;
    assert!(ui.bounds("flux-events-row-1").is_some());
    assert!(ui.bounds("flux-events-row-2").is_none());
    ui.press("escape")?;
    ui.click("flux-events-row-0")?;
    assert_eq!(
        opened_flux(&ui),
        Some((flock_core::FluxKind::Kustomization, "apps-payments".into()))
    );
    Ok(())
}

#[test]
fn kustomization_drift_lists_changed_fields_and_opens_the_object() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Kustomization)?;
    assert!(ui.bounds("flux-drift").is_some());
    ui.click("flux-drift-object-0")?;
    let opened = ui.state().location().resource.clone();
    assert_eq!(
        opened.map(|target| (target.kind, target.name)),
        Some(("Deployment".to_owned(), "checkout-api".to_owned()))
    );
    Ok(())
}

#[test]
fn page_shortcuts_work_on_a_page_opened_from_the_palette() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.press("cmd-k")?;
    ui.type_text("checkout-api")?;
    ui.press("enter")?;
    assert_eq!(
        ui.state().workload().map(|target| target.name.clone()),
        Some("checkout-api".to_owned())
    );
    ui.press("cmd-y")?;
    assert!(ui.bounds("workload-edit-yaml").is_some());
    ui.press("cmd-e")?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    Ok(())
}

#[test]
fn command_e_edits_the_yaml_on_detail_and_flux_pages() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Namespace)?;
    ui.press("cmd-e")?;
    let edited = ui.state().location().clone();
    assert_eq!(edited.page, Page::EditYaml);
    assert_eq!(
        edited.resource.map(|target| target.name),
        Some("payments".to_owned())
    );
    let mut ui = Ui::open(Screen::Kustomization)?;
    ui.press("cmd-e")?;
    assert_ne!(ui.state().location().page, Page::EditYaml);
    ui.press("cmd-y cmd-e")?;
    let edited = ui.state().location().clone();
    assert_eq!(edited.page, Page::EditYaml);
    assert_eq!(
        edited.resource.map(|target| target.kind),
        Some("Kustomization".to_owned())
    );
    Ok(())
}

fn editor_text(ui: &mut Ui) -> anyhow::Result<String> {
    ui.press("cmd-a")?;
    ui.press("cmd-c")?;
    Ok(ui.clipboard().unwrap_or_default())
}

#[test]
fn the_yaml_editor_indents_with_brackets_and_goes_back_with_control_minus_or_escape()
-> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Namespace)?;
    let detail = ui.state().location().clone();
    ui.press("cmd-e")?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    ui.click("edit-editor")?;
    let before = editor_text(&mut ui)?;
    ui.press("cmd-]")?;
    let indented = editor_text(&mut ui)?;
    assert_ne!(indented, before);
    assert_eq!(ui.state().location().page, Page::EditYaml);
    ui.press("cmd-[")?;
    assert_eq!(editor_text(&mut ui)?, before);
    ui.press("ctrl--")?;
    assert_eq!(ui.state().location(), &detail);
    ui.press("ctrl-_")?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    ui.press("ctrl--")?;
    ui.press("cmd-e")?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    ui.click("edit-editor")?;
    ui.press("escape")?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    ui.press("cmd-[")?;
    assert_eq!(ui.state().location(), &detail);
    Ok(())
}

#[test]
fn flux_events_are_reachable_from_the_keyboard() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::FluxEvents)?;
    ui.press("cmd-e down enter")?;
    assert!(opened_flux(&ui).is_some());
    Ok(())
}

#[test]
fn an_unsaved_yaml_edit_survives_switching_tabs() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::EditYaml)?;
    ui.click("edit-editor")?;
    ui.press("cmd-a")?;
    ui.type_text("{apiVersion: v1, kind: ConfigMap, metadata: {name: checkout-api-config, namespace: payments}, data: {level: debug}}")?;
    ui.press("cmd-t")?;
    assert_eq!(ui.state().location().page, Page::Overview);
    ui.press("cmd-1")?;
    assert_eq!(ui.state().location().page, Page::EditYaml);
    ui.click("edit-review")?;
    assert!(ui.bounds("edit-review-view").is_some());
    Ok(())
}

const CUSTOM_RESOURCES_ROW: &str = "nav-custom resources";

#[test]
fn the_sidebar_row_opens_the_definitions_page_and_back_returns_to_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let definitions = ui.state().snapshot.definitions(&ClusterId::new(SAMPLE));
    assert_ne!(definitions.len(), 0);
    ui.scroll("sidebar-nav", gpui::point(gpui::px(0.), gpui::px(-2000.)))?;
    ui.click(CUSTOM_RESOURCES_ROW)?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::CustomResources);
    assert_eq!(location.crd, None);
    assert!(ui.bounds("crds-row-0").is_some());
    ui.press("/")?;
    ui.type_text("issuer")?;
    assert!(ui.bounds("crds-row-1").is_some());
    assert!(ui.bounds("crds-row-2").is_none());
    ui.click("crds-row-1")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::CustomResources);
    assert_eq!(
        location.crd,
        Some(flock_core::crd::CrdKey::new("cert-manager.io", "Issuer"))
    );
    assert!(ui.bounds("empty-state-cluster").is_some());
    let sentence = ui.read(Shell::custom_empty_sentence).unwrap_or_default();
    assert!(sentence.starts_with("No Issuers in "), "{sentence}");
    ui.press("cmd-[")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::CustomResources);
    assert_eq!(location.crd, None);
    assert!(ui.bounds("crds-row-0").is_some());
    Ok(())
}

#[test]
fn the_keyboard_opens_a_definition_from_the_definitions_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::CustomResourceDefinitions)?;
    ui.press("/")?;
    ui.type_text("clusterissuer")?;
    ui.press("cmd-e down enter")?;
    let location = ui.state().location().clone();
    assert_eq!(
        location.crd,
        Some(flock_core::crd::CrdKey::new(
            "cert-manager.io",
            "ClusterIssuer"
        ))
    );
    assert!(
        ui.bounds("custom-clusterissuers.cert-manager.io-row-0")
            .is_some()
    );
    assert!(ui.bounds("custom-namespace").is_none());
    Ok(())
}

#[test]
fn a_flux_definition_opens_its_flux_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::CustomResourceDefinitions)?;
    ui.press("/")?;
    ui.type_text("gitrepositories")?;
    assert!(ui.bounds("crds-row-1").is_none());
    ui.click("crds-row-0")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::Sources);
    assert_eq!(location.crd, None);
    Ok(())
}

#[test]
fn a_custom_object_opens_the_inspector_and_edits_its_yaml() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::CustomResources)?;
    assert!(ui.bounds("custom-namespace").is_some());
    assert!(ui.bounds("custom-inspector-pane").is_none());
    ui.click("custom-certificates.cert-manager.io-row-1")?;
    assert!(ui.bounds("custom-inspector-pane").is_some());
    ui.click("edit-custom")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::EditYaml);
    let target = location.resource.map(|target| (target.kind, target.name));
    assert_eq!(
        target,
        Some((
            "Certificate.cert-manager.io".into(),
            "payments-db-tls".into()
        ))
    );
    Ok(())
}

#[test]
fn helm_releases_open_their_history_values_and_flux_owner() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::HelmReleases)?;
    assert_eq!(ui.state().location().page, Page::Releases);
    assert!(ui.bounds("helm-releases-row-2").is_some());
    ui.press("/")?;
    ui.type_text("airflow")?;
    assert!(ui.bounds("helm-releases-row-1").is_none());
    ui.press("escape")?;
    ui.click("helm-releases-row-0")?;
    let location = ui.state().location().clone();
    let opened = location.helm.map(|key| key.name);
    assert_eq!(opened.as_deref(), Some("kube-prometheus-stack"));
    assert!(ui.bounds("helm-history-row-2").is_some());
    assert!(ui.bounds("helm-user-values").is_some());
    ui.click("helm-values-computed")?;
    assert!(ui.bounds("helm-computed-values").is_some());
    assert!(ui.bounds("helm-user-values").is_none());
    ui.click("helm-open-flux")?;
    let flux = ui
        .state()
        .location()
        .flux_object
        .clone()
        .map(|key| key.kind);
    assert_eq!(flux, Some(flock_core::FluxKind::HelmRelease));
    Ok(())
}

fn device_span(bounds: gpui::Bounds<Pixels>) -> anyhow::Result<(u32, u32, u32, u32)> {
    const SCALE: f32 = 2.;
    let device = bounds.to_device_pixels(SCALE);
    let left = u32::try_from(i32::from(device.origin.x))?;
    let top = u32::try_from(i32::from(device.origin.y))?;
    let width = u32::try_from(i32::from(device.size.width))?;
    let height = u32::try_from(i32::from(device.size.height))?;
    Ok((left, top, left + width, top + height))
}

#[test]
fn end_aligned_cells_with_tooltips_stay_end_aligned() -> anyhow::Result<()> {
    const INK: u8 = 40;
    const INSET: u32 = 12;
    let mut ui = Ui::open(Screen::Namespaces)?;
    let (_, _, pods_edge, _) = device_span(bounds_of(&mut ui, "namespaces-resize-2")?)?;
    let (_, _, cpu_edge, _) = device_span(bounds_of(&mut ui, "namespaces-resize-3")?)?;
    let (_, top, _, bottom) = device_span(bounds_of(&mut ui, "namespaces-row-2")?)?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("namespaces.png");
    ui.save_png(&path)?;
    let image = image::open(&path)?.to_rgba8();
    let (left, right) = (pods_edge + INSET, cpu_edge - INSET);
    let (top, bottom) = (top + INSET, bottom - INSET);
    let background = image.get_pixel(left, top).0;
    let inked = |x: u32| {
        (top..bottom).any(|y| {
            let pixel = image.get_pixel(x, y).0;
            pixel
                .iter()
                .zip(background.iter())
                .any(|(a, b)| a.abs_diff(*b) > INK)
        })
    };
    let ink: Vec<u32> = (left..right).filter(|&x| inked(x)).collect();
    assert!(!ink.is_empty(), "the CPU cell drew nothing");
    let gap = right - ink.last().copied().unwrap_or(left);
    assert!(
        gap <= INSET,
        "CPU text should hug the right edge of its column: ink {:?}..{:?}, edge {right}",
        ink.first(),
        ink.last()
    );
    Ok(())
}

#[test]
fn cluster_problems_group_by_cause_and_rows_open_the_object() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("nav-problems")?;
    assert_eq!(ui.state().location().page, Page::ClusterProblems);
    assert!(ui.bounds("problem-template").is_some());
    assert!(ui.bounds("problem-histogram").is_some());
    ui.click("problem-cause-1")?;
    ui.click("problem-affected-row-0")?;
    let state = ui.state();
    let opened = state
        .location()
        .resource
        .clone()
        .context("the row opens its object")?;
    assert_eq!(opened.cluster, ClusterId::new("prod-us-east"));
    assert_eq!(state.location().page, page_for_kind(&opened.kind));
    Ok(())
}

#[test]
fn the_status_bar_shows_only_the_problem_counts_and_the_kubernetes_version() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let problems = ui
        .bounds("status-problems")
        .context("the problem counts render")?;
    let version = ui
        .bounds("status-version")
        .context("a cluster page shows its Kubernetes version")?;
    assert_eq!(version.left() - problems.right(), flock_ui::theme::SPACE_8);
    let mut ui = Ui::open(Screen::Flock)?;
    assert!(ui.bounds("status-problems").is_some());
    assert!(ui.bounds("status-version").is_none());
    Ok(())
}

#[test]
fn the_status_bar_counts_open_the_fleet_problems_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("status-problems")?;
    let location = ui.state().location().clone();
    assert_eq!(location.page, Page::Problems);
    assert_eq!(location.cluster, None);
    Ok(())
}

#[test]
fn the_status_bar_refresh_button_is_clickable_and_keeps_the_page() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let before = ui.state().location().clone();
    assert!(ui.bounds("status-refresh").is_some());
    ui.click("status-refresh")?;
    ui.press("cmd-alt-r")?;
    assert_eq!(ui.state().location(), &before);
    assert!(ui.bounds("status-refresh").is_some());
    Ok(())
}

#[test]
fn the_api_server_row_opens_the_5xx_drill_in() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::Cluster, flock::Preset::default(), appearance)?;
        assert!(ui.bounds("overview-api-drill-close").is_none());
        scroll_overview_to_the_end(&mut ui)?;
        ui.save_png(&dir.join(format!("overview-control-plane-{appearance:?}.png")))?;
        ui.click("overview-control-api-server")?;
        assert!(ui.bounds("overview-api-drill-close").is_some());
        assert!(ui.bounds("overview-api-drill-request-4").is_some());
        ui.save_png(&dir.join(format!("overview-api-drill-{appearance:?}.png")))?;
        ui.click("overview-api-drill-codes-1")?;
        assert!(ui.bounds("overview-api-drill-request-1").is_some());
        assert!(ui.bounds("overview-api-drill-request-2").is_none());
        ui.press("escape")?;
        assert!(ui.bounds("overview-api-drill-close").is_none());
        assert_eq!(ui.state().location().page, Page::ClusterOverview);
    }
    Ok(())
}

#[test]
fn a_saved_png_shows_the_tabs_restored_into_the_ui() -> anyhow::Result<()> {
    use flock_core::history::Location;
    let dir = tempfile::tempdir()?;
    let restored_path = dir.path().join("restored.png");
    let home_path = dir.path().join("home.png");
    let mut restored = Ui::open(Screen::Flock)?;
    let nodes = Location::page_on(ClusterId::new("prod-us-east"), Page::Nodes);
    restored.restore(&flock_shots::one_tab(nodes))?;
    restored.save_png(&restored_path)?;
    Ui::open(Screen::Flock)?.save_png(&home_path)?;
    let restored_image = image::open(&restored_path)?.to_rgba8();
    let home_image = image::open(&home_path)?.to_rgba8();
    assert_ne!(restored_image, home_image);
    Ok(())
}

#[test]
fn the_expiring_horizon_stays_inside_its_column_with_the_drill_in_open() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    scroll_overview_to_the_end(&mut ui)?;
    ui.click("overview-control-api-server")?;
    let last = ui
        .bounds("overview-expiring-horizon-3")
        .ok_or_else(|| anyhow::anyhow!("missing the last horizon"))?;
    let network = ui
        .bounds("overview-fact-services-without-endpoints")
        .ok_or_else(|| anyhow::anyhow!("missing the network column"))?;
    assert!(last.origin.x + last.size.width <= network.origin.x);
    Ok(())
}

#[test]
fn the_shell_button_is_disabled_in_place_when_no_container_runs() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Pods)?;
    let enabled = bounds_of(&mut ui, "pod-shell")?;
    ui.click("pod-failing-elsewhere")?;
    ui.click("pod-filter-chips-3")?;
    ui.click("pods-row-0")?;
    let disabled = bounds_of(&mut ui, "pod-shell")?;
    assert_eq!(disabled, enabled);
    ui.click("pod-shell")?;
    assert!(ui.bounds("pod-close-shell").is_none());
    Ok(())
}

#[test]
fn the_pod_error_block_sits_above_the_details_and_copies_its_exact_text() -> anyhow::Result<()> {
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::Pods)?;
    let block = bounds_of(&mut ui, "error-block-0")?;
    let facts = top(&mut ui, "pod-container-head")?;
    assert!(block.bottom() < facts);
    let wrong = bounds_of(&mut ui, "error-block-0-message")?;
    let from = wrong.origin + point(-px(SELECT_INSET), wrong.size.height - px(SELECT_INSET));
    let to = point(wrong.right(), wrong.bottom()) - point(px(SELECT_INSET), px(SELECT_INSET));
    ui.drag_between(from, to)?;
    ui.press("cmd-c")?;
    assert_eq!(ui.clipboard().as_deref(), Some("last exit Error (1)"));
    Ok(())
}

#[test]
fn a_raw_message_keeps_its_size_when_hovered_and_selected() -> anyhow::Result<()> {
    use gpui::{point, px};
    let mut ui = Ui::open(Screen::Pods)?;
    let resting = bounds_of(&mut ui, "error-block-0-message")?;
    ui.hover("error-block-0-message")?;
    assert_eq!(bounds_of(&mut ui, "error-block-0-message")?, resting);
    let from = resting.origin + point(px(SELECT_INSET), px(SELECT_INSET));
    let to = point(resting.right(), resting.bottom()) - point(px(SELECT_INSET), px(SELECT_INSET));
    ui.drag_between(from, to)?;
    assert_eq!(bounds_of(&mut ui, "error-block-0-message")?, resting);
    Ok(())
}

const SELECT_INSET: f32 = 4.;

#[test]
fn filter_terms_are_highlighted_in_event_message_cells() -> anyhow::Result<()> {
    const NEAR: f32 = 0.1;
    let mut ui = Ui::open(Screen::Events)?;
    let accent = ui.read_app(|cx| {
        use flock_ui::theme::ActiveTheme;
        cx.palette().acc
    });
    let rgba = accent.to_rgb();
    let target = [rgba.r, rgba.g, rgba.b];
    let dir = tempfile::tempdir()?;
    let accent_pixels = |ui: &mut Ui, name: &str| -> anyhow::Result<usize> {
        let (left, top, right, bottom) = device_span(bounds_of(ui, "events-row-0")?)?;
        let path = dir.path().join(name);
        ui.save_png(&path)?;
        let image = image::open(&path)?.to_rgba8();
        let count = (top..bottom)
            .flat_map(|y| (left..right).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let pixel = image.get_pixel(x, y).0;
                (0..3).all(|ix| (f32::from(pixel[ix]) / 255. - target[ix]).abs() <= NEAR)
            })
            .count();
        Ok(count)
    };
    let before = accent_pixels(&mut ui, "before.png")?;
    ui.press("/")?;
    ui.type_text("restarting")?;
    let after = accent_pixels(&mut ui, "after.png")?;
    assert!(after > before, "no highlight drawn: {before} -> {after}");
    Ok(())
}

#[test]
fn filter_placeholder_stays_left_of_the_shortcut_hint_and_leaves_when_typing() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Nodes)?;
    let placeholder = ui.bounds("node-filter-placeholder");
    let shortcut = ui.bounds("node-filter-shortcut");
    let (Some(placeholder), Some(shortcut)) = (placeholder, shortcut) else {
        anyhow::bail!("placeholder or shortcut hint missing");
    };
    assert!(placeholder.right() <= shortcut.left());
    ui.press("/")?;
    ui.type_text("a")?;
    assert!(ui.bounds("node-filter-placeholder").is_none());
    Ok(())
}

#[test]
fn a_nav_row_with_three_badges_keeps_its_full_label() -> anyhow::Result<()> {
    let (mut ui, updates) = Ui::scripted(Screen::Cluster)?;
    let mut snapshot = flock_demo::snapshot();
    let counts = flock_core::Counts::total(3)
        .with_err(1)
        .with_warn(1)
        .with_sus(1);
    for cluster in &mut snapshot.clusters {
        cluster.nav.insert(Page::HelmReleases, counts);
    }
    updates.try_send(flock_core::Update::Snapshot(Box::new(snapshot)))?;
    ui.settle()?;
    let label = Page::HelmReleases.label();
    let runs = [TextRun {
        len: label.len(),
        font: font(FONT_SANS),
        color: rgb(0).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let text_width = ui.read_in_window(|_, window, _| {
        window
            .text_system()
            .shape_line(SharedString::from(label), TEXT_UI, &runs, None)
            .width
    });
    let shown = width(&mut ui, "nav-helmreleases-label").context("the label is drawn")?;
    assert!(
        shown >= text_width,
        "{shown:?} clips {label} at {text_width:?}"
    );
    for badge in ["err", "warn", "suspended"] {
        let selector: &'static str = format!("nav-helmreleases-{badge}").leak();
        assert!(ui.bounds(selector).is_some(), "{selector} is missing");
    }
    Ok(())
}
