use flock::Screen;
use flock_core::resources::EVENT_LIST;
use flock_core::{ClusterId, ClusterSummary, Command, Page};
use flock_shots::Ui;
use flock_ui::Shell;
use flock_ui::theme::{
    ICON_BUTTON, SIDEBAR, SPACE_3, TAB_ENTER, TAB_LEAVE, TITLE_ROW, TRAFFIC_LIGHTS_INSET,
};
use gpui::Pixels;

const EXPECTED_WIDTH: u32 = 2880;
const EXPECTED_HEIGHT: u32 = 1800;
const TAB_STRIP_REWIND: f32 = 1000.;

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

#[test]
fn the_sidebar_buttons_collapse_and_expand_it() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("collapse-sidebar")?;
    assert!(!ui.state().sidebar_open);
    ui.click("expand-sidebar")?;
    assert!(ui.state().sidebar_open);
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
    assert!(ui.bounds("switcher-back").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("switcher-back").is_none());
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
    ui.click("workload-actions-item-4")?;
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
    assert!(ui.bounds("resource-filter-clear").is_some());
    ui.press("escape")?;
    assert_eq!(filter_words(&ui), Some(Vec::new()));
    assert!(
        ui.bounds(selector(format!("{DEPLOYMENTS}-row-1")))
            .is_some()
    );
    assert!(ui.bounds("resource-filter-clear").is_none());
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
    ui.click(selector(format!("{DEPLOYMENTS}-row-menu-item-1")))?;
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
    assert!(ui.bounds("close-node").is_some());
    ui.press("escape")?;
    assert!(ui.bounds("close-node").is_none());
    assert_eq!(ui.state().location().page, Page::Nodes);
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
    for (selector, page) in [
        ("overview-pods", Page::Pods),
        ("overview-nodes", Page::Nodes),
        ("overview-deployments", Page::Deployments),
        ("overview-statefulsets", Page::StatefulSets),
        ("overview-daemonsets", Page::DaemonSets),
        ("overview-jobs", Page::Jobs),
        ("overview-cronjobs", Page::CronJobs),
    ] {
        let mut ui = Ui::open(Screen::Cluster)?;
        let cluster = ui.state().location().cluster.clone();
        ui.click(selector)?;
        assert_eq!(ui.state().location().page, page);
        assert_eq!(ui.state().location().cluster, cluster);
    }
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
    assert_eq!(pods.origin.x + pods.size.width + RULE, nodes.origin.x);
    assert!(pods.origin.x > memory.origin.x);
    Ok(())
}

#[test]
fn overview_sections_accept_keyboard_activation() -> anyhow::Result<()> {
    for key in ["enter", "space"] {
        let mut ui = Ui::open(Screen::Cluster)?;
        ui.press("cmd-e")?;
        ui.press(key)?;
        assert_eq!(ui.state().location().page, Page::Pods);
    }
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

#[test]
fn node_inspector_shows_cpu_and_memory_commitments() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Nodes)?;
    ui.click("nodes-row-0")?;
    assert!(ui.bounds("node-capacity-CPU").is_some());
    assert!(ui.bounds("node-capacity-Memory").is_some());
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
fn flux_pages_list_their_objects_and_open_one_on_double_click() -> anyhow::Result<()> {
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
    ui.double_click("flux-list-row-0")?;
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
fn pod_logs_have_no_previous_container_toggle() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::PodsShell)?;
    ui.click("dock-tab-0")?;
    assert!(ui.bounds("pod-logs-follow").is_some());
    assert!(ui.bounds("pod-logs-previous").is_none());
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
    assert!(nodes.bounds("node-primary").is_some());
    assert!(nodes.bounds("node-actions").is_none());
    nodes.right_click("nodes-row-1")?;
    assert!(nodes.bounds("nodes-row-menu-item-0").is_some());
    nodes.click("nodes-row-menu-item-2")?;
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
    ui.press("cmd-w")?;
    assert_eq!(tab_pages(&ui), [Page::Overview]);
    assert_eq!(ui.read_app(|cx| cx.windows().len()), 1);
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
fn the_switcher_gear_opens_settings_and_right_click_offers_connection_actions() -> anyhow::Result<()>
{
    let mut ui = Ui::open(Screen::Flock)?;
    ui.click("cluster-switcher")?;
    let staging = switcher_row(&ui, "staging")?;
    ui.right_click(staging)?;
    assert!(ui.bounds("switcher-back").is_some());
    assert!(ui.bounds("switcher-action-2").is_some());
    ui.press("escape escape")?;
    assert!(!ui.read(Shell::switcher_open));
    ui.click("cluster-switcher")?;
    ui.hover(staging)?;
    let gear: &'static str = staging.replace("row", "gear").leak();
    ui.click(gear)?;
    assert_eq!(ui.state().location().page, Page::ClusterSettings);
    assert_eq!(settings_selection(&ui), Some(ClusterId::new("staging")));
    assert!(!ui.read(Shell::switcher_open));
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
    let start = point(border.center().x + px(12.), row.center().y);
    ui.drag_between(start, start + point(px(40.), px(0.)))?;
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
        let start = point(border.center().x + px(12.), row.center().y);
        ui.drag_between(start, start + point(px(40.), px(0.)))?;
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
    ui.click("collapse-sidebar")?;
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
    assert!(ui.bounds("nav-deployments-total").is_some());
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
    let slot = ui.bounds("tab-slot-1").map(|bounds| bounds.size.width);
    let natural = ui.bounds("tab-1").map(|bounds| bounds.size.width);
    assert!(slot.is_some());
    assert_eq!(slot, natural);
    assert_eq!(tab_gap(&mut ui), Some(gpui::px(0.)));
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
fn every_utilization_window_keeps_the_chart_and_local_collection_note() -> anyhow::Result<()> {
    let mut ui = Ui::with_state(Screen::Cluster, flock_shots::StateFixture::MetricsLocal)?;
    for selector in ["range-0", "range-1", "range-2", "range-3"] {
        ui.click(selector)?;
        assert!(ui.bounds("utilization-chart").is_some());
        assert!(ui.bounds("metrics-note").is_some(), "{selector}");
        assert!(ui.bounds("metrics-empty").is_none(), "{selector}");
    }
    Ok(())
}

#[test]
fn empty_utilization_opens_the_selected_clusters_metrics_settings() -> anyhow::Result<()> {
    let mut ui = Ui::with_state(Screen::Cluster, flock_shots::StateFixture::MetricsEmpty)?;
    ui.click("range-1")?;
    assert!(ui.bounds("utilization-chart").is_some());
    assert!(ui.bounds("metrics-empty").is_some());
    assert!(ui.bounds("metrics-note").is_none());
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

fn first_warning(ui: &Ui) -> anyhow::Result<flock_core::events::Event> {
    let state = ui.state();
    let cluster = state
        .location()
        .cluster
        .clone()
        .ok_or_else(|| anyhow::anyhow!("cluster"))?;
    let detail = state
        .snapshot
        .detail(&cluster)
        .ok_or_else(|| anyhow::anyhow!("detail"))?;
    detail
        .recent_warnings(1)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("the demo cluster has no warnings"))
}

#[test]
fn overview_warning_message_shows_in_a_tooltip_on_hover() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    assert!(ui.bounds("tooltip").is_none());
    ui.hover("overview-warnings-row-0")?;
    ui.advance_clock(std::time::Duration::from_secs(1))?;
    assert!(ui.bounds("tooltip").is_some());
    Ok(())
}

#[test]
fn overview_warning_row_opens_the_event_inspector_with_the_full_message() -> anyhow::Result<()> {
    for appearance in [
        flock_ui::theme::Appearance::Dark,
        flock_ui::theme::Appearance::Light,
    ] {
        let mut ui = Ui::with(Screen::Cluster, flock::Preset::default(), appearance)?;
        assert!(ui.bounds("event-inspector-pane").is_none());
        ui.click("overview-warnings-row-0")?;
        assert!(ui.bounds("event-inspector-pane").is_some());
        assert!(ui.bounds("event-message").is_some());
        assert_eq!(ui.state().location().page, Page::ClusterOverview);
        ui.press("escape")?;
        assert!(ui.bounds("event-inspector-pane").is_none());
        ui.press("down enter")?;
        assert!(ui.bounds("event-inspector-pane").is_some());
        ui.click("close-event")?;
        assert!(ui.bounds("event-inspector-pane").is_none());
    }
    Ok(())
}

#[test]
fn overview_warning_object_link_opens_the_resource_without_opening_the_inspector()
-> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    let object = first_warning(&ui)?.object;
    let selector = selector(format!("warning-object-{object}"));
    ui.click(selector)?;
    assert_ne!(ui.state().location().page, Page::ClusterOverview);
    Ok(())
}

#[test]
fn all_events_opens_the_events_page_on_the_warnings_chip() -> anyhow::Result<()> {
    let mut ui = Ui::open(Screen::Cluster)?;
    ui.click("all-events")?;
    assert_eq!(ui.state().location().page, Page::Events);
    let chip = ui.read(|shell, cx| shell.filter_bar(cx).map(|bar| bar.read(cx).chip()));
    assert_eq!(chip, Some(1));
    Ok(())
}
