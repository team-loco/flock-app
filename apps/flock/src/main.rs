#[cfg(all(target_os = "macos", not(test)))]
mod menubar;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Context as _;
use clap::Parser;
use flock::{
    APP_ID, APP_NAME, AppearanceArg, Object, Preset, Screen, WINDOW_HEIGHT, WINDOW_WIDTH,
    bind_keys, bundled, menus, parse_object, root_view,
};
use flock_core::Status;
use flock_core::tabs::SavedWindow;
use flock_core::{Backend, ClusterId, Command, Receiver, Sender, UpdaterRequest, UpdaterStatus};
use flock_demo::DemoBackend;
use flock_kube::LiveBackend;
use flock_kube::app_settings::SettingsStore;
use flock_kube::discovery::DiscoverySources;
use flock_kube::settings::KubeSettings;
use flock_kube::store::ClusterStore;
use flock_kube::tab_store::TabStore;
use flock_ui::actions::{NewWindow, Quit};
use flock_ui::kit::{Toast, toast};
use flock_ui::theme::{Appearance, TRAFFIC_LIGHT_X, TRAFFIC_LIGHT_Y};
use flock_ui::{NewWindowRequest, Root, Shell, Store, assets};
use flock_update::{GitHub, Updater, signature::PUBLIC_KEY};
use gpui::{
    App, AppContext as _, Bounds, Size, TitlebarOptions, WindowBounds, WindowOptions, point, px,
    size,
};
use tracing_subscriber::EnvFilter;

const MIN_WIDTH: f32 = 720.;
const MIN_HEIGHT: f32 = 480.;
const LOG_SNAPSHOTS_VAR: &str = "FLOCK_LOG_SNAPSHOTS";
const SETTINGS_SET_ASIDE: &str = "Settings could not be read";
const TABS_SET_ASIDE: &str = "Open tabs could not be read";

#[derive(Default)]
struct Stores {
    settings: Option<SettingsStore>,
    tabs: Option<TabStore>,
}
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const BUNDLE_EXTENSION: &str = ".app";
const UPDATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
const MAX_UPDATE_ZIP: u64 = 256 << 20;
const MAX_UPDATE_TEXT: u64 = 4 << 10;

type UpdaterChannels = (Sender<UpdaterRequest>, Receiver<UpdaterStatus>);

#[derive(Parser, Debug)]
#[command(
    name = "flock",
    about = "Watch many Kubernetes clusters at once",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("FLOCK_GIT_SHA"), ")")
)]
struct Args {
    #[arg(long)]
    demo: bool,
    #[arg(long, value_enum, default_value_t = Screen::Flock)]
    screen: Screen,
    #[arg(long, value_enum)]
    appearance: Option<AppearanceArg>,
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, value_name = "NAME")]
    cluster: Option<String>,
    #[arg(long, value_name = "NAMESPACE/NAME", value_parser = parse_object)]
    object: Option<Object>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    let (update_tx, update_rx) = async_channel::unbounded();
    let (command_tx, command_rx) = async_channel::unbounded();
    let sources = DiscoverySources::from_env();
    if args.data_dir.is_none()
        && let Some(home) = &sources.home
    {
        flock_kube::migrate::previous_app_dirs(home);
    }
    let dir = data_dir(args.data_dir.clone(), &sources);
    let settings = dir
        .as_ref()
        .ok()
        .map(|dir| {
            SettingsStore::open(dir)
                .with_context(|| format!("reading settings in {}", dir.display()))
        })
        .transpose()?;
    let refresh = settings
        .as_ref()
        .map(|store| store.settings().refresh)
        .unwrap_or_default();
    let prefs_dir = if args.demo {
        None
    } else {
        dir.as_ref().ok().cloned()
    };
    let tabs = prefs_dir
        .as_ref()
        .map(|dir| {
            TabStore::open(dir).with_context(|| format!("reading open tabs in {}", dir.display()))
        })
        .transpose()?;
    let backend: Box<dyn Backend> = if args.demo {
        Box::new(DemoBackend)
    } else {
        let dir = dir?;
        let store = ClusterStore::open(&dir)
            .with_context(|| format!("reading saved clusters in {}", dir.display()))?;
        let kube = KubeSettings::with_refresh(&refresh);
        Box::new(LiveBackend::new(kube, sources, store))
    };
    backend.start(update_tx, command_rx);
    let update_rx = if std::env::var_os(LOG_SNAPSHOTS_VAR).is_some() {
        log_snapshots(update_rx)
    } else {
        update_rx
    };

    let forced = args.appearance.map(Appearance::from);
    let screen = args.screen;
    let preset = Preset {
        cluster: args.cluster.map(ClusterId::new),
        object: args.object,
    };
    let self_update = start_updater();
    let failure = Rc::new(RefCell::new(None));
    let failed = failure.clone();
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            flock_ui::prefs::init(prefs_dir.as_deref(), cx);
            let launched = launch(
                cx,
                forced,
                Stores { settings, tabs },
                self_update,
                update_rx,
                command_tx,
                screen,
                &preset,
            );
            if let Err(error) = launched {
                failed.replace(Some(error));
                cx.quit();
            }
        });
    failure.take().map_or(Ok(()), Err)
}

fn log_snapshots(updates: Receiver<flock_core::Update>) -> Receiver<flock_core::Update> {
    let (forward_tx, forward_rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        while let Ok(update) = updates.recv_blocking() {
            if let flock_core::Update::Snapshot(snapshot) = &update {
                let connected = snapshot
                    .clusters
                    .iter()
                    .filter(|c| c.is_connected())
                    .count();
                let problems = &snapshot.home.problems;
                tracing::info!(
                    clusters = snapshot.clusters.len(),
                    connected,
                    problems = problems.len(),
                    "snapshot"
                );
                for problem in problems {
                    let target = &problem.target;
                    tracing::info!(
                        cluster = %target.cluster.as_str(),
                        kind = %target.kind,
                        namespace = target.namespace.as_deref().unwrap_or_default(),
                        name = %target.name,
                        "problem"
                    );
                }
            }
            if forward_tx.send_blocking(update).is_err() {
                break;
            }
        }
    });
    forward_rx
}

fn start_updater() -> Option<UpdaterChannels> {
    let started = updater_config().and_then(|config| {
        let user_agent = format!("{APP_NAME}/{VERSION}");
        let source = GitHub::new(REPOSITORY, UPDATE_TIMEOUT, &user_agent)?;
        let relaunch: flock_update::Relaunch = Box::new(flock_update::install::relaunch);
        let updater = Updater::new(source, config, relaunch);
        let (request_tx, request_rx) = async_channel::unbounded();
        let (status_tx, status_rx) = async_channel::unbounded();
        flock_update::spawn(updater, request_rx, status_tx)
            .map_err(flock_update::Error::io("updater thread"))?;
        Ok((request_tx, status_rx))
    });
    started
        .inspect_err(|error| tracing::warn!(%error, "update checks are off"))
        .ok()
}

fn updater_config() -> flock_update::Result<flock_update::Config> {
    let target = std::env::current_exe().map_or(flock_update::Target::Manual, |exe| {
        flock_update::install::target(&exe)
    });
    Ok(flock_update::Config {
        current: flock_update::version::parse(VERSION)?,
        identifier: APP_ID.to_owned(),
        bundle_name: format!("{APP_NAME}{BUNDLE_EXTENSION}"),
        target,
        public_key: PUBLIC_KEY.to_owned(),
        max_download: MAX_UPDATE_ZIP,
        max_text: MAX_UPDATE_TEXT,
    })
}

fn data_dir(data_dir: Option<PathBuf>, sources: &DiscoverySources) -> anyhow::Result<PathBuf> {
    match (data_dir, &sources.home) {
        (Some(dir), _) => Ok(dir),
        (None, Some(home)) => Ok(ClusterStore::default_dir(home)),
        (None, None) => anyhow::bail!("no home directory; pass --data-dir"),
    }
}

fn install_settings(store: SettingsStore, cx: &mut App) {
    let set_aside = store.set_aside().map(|kept| kept.display().to_string());
    let initial = store.settings().clone();
    let store = Rc::new(RefCell::new(store));
    let save: flock_ui::settings::Saver = Rc::new(move |settings| {
        store
            .borrow_mut()
            .save(settings)
            .map_err(|error| error.to_string())
    });
    let is_bundled = std::env::current_exe().is_ok_and(|exe| bundled(&exe));
    flock_ui::settings::install(initial, save, is_bundled, cx);
    if let Some(kept) = set_aside {
        let notice = Toast::new(Status::Warn, SETTINGS_SET_ASIDE).detail(format!("Kept as {kept}"));
        toast(notice, cx);
    }
}

#[allow(clippy::too_many_arguments)]
fn launch(
    cx: &mut App,
    forced: Option<Appearance>,
    stores: Stores,
    self_update: Option<UpdaterChannels>,
    updates: Receiver<flock_core::Update>,
    commands: Sender<Command>,
    screen: Screen,
    preset: &Preset,
) -> anyhow::Result<()> {
    flock_ui::init(forced, cx)?;
    cx.set_app_identity(APP_ID, APP_NAME);
    if let Some(settings) = stores.settings {
        install_settings(settings, cx);
    }
    let restored = stores
        .tabs
        .map(|tabs| install_tabs(tabs, cx))
        .unwrap_or_default();
    if let Some((requests, statuses)) = self_update {
        flock_ui::updater::install(requests, statuses, cx);
    }
    bind_keys(cx);
    cx.set_menus(menus());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() && !flock_ui::settings::current(cx).keeps_running_in_menu_bar() {
            cx.quit();
        }
    })
    .detach();
    let store = cx.new(|cx| Store::new(updates, commands, cx));
    #[cfg(all(target_os = "macos", not(test)))]
    menubar::install(store.clone(), cx);
    let new_window_store = store.clone();
    cx.on_action(move |_: &NewWindow, cx| {
        if let Err(error) = open_flock_window(new_window_store.clone(), Vec::new(), None, cx) {
            tracing::error!(%error, "opening a Flock window");
        }
    });
    if restored.is_empty() || !matches!(screen, Screen::Flock) {
        return open_startup_window(store, screen, preset, cx);
    }
    for saved in restored {
        open_flock_window(store.clone(), Vec::new(), Some(&saved), cx)?;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn bring_to_front(store: gpui::Entity<Store>, command: Option<Command>, cx: &mut App) {
    let front = cx
        .window_stack()
        .and_then(|stack| stack.first().copied())
        .or_else(|| cx.windows().first().copied());
    let shell = front
        .and_then(|handle| handle.downcast::<Root>())
        .and_then(|root| root.read(cx).ok().map(|root| root.view().clone()))
        .and_then(|view| view.downcast::<Shell>().ok());
    let (Some(front), Some(shell)) = (front, shell) else {
        let initial = command.into_iter().collect();
        if let Err(error) = open_flock_window(store, initial, None, cx) {
            tracing::error!(%error, "opening a Flock window");
        }
        return;
    };
    let shown = front.update(cx, |_, window, cx| {
        if let Some(command) = command {
            shell.update(cx, |shell, cx| {
                shell.dispatch(Command::NewTab(Box::new(command)), cx);
            });
        }
        window.activate_window();
    });
    if let Err(error) = shown {
        tracing::error!(%error, "bringing a Flock window to the front");
    }
    cx.activate(true);
}

fn install_tabs(store: TabStore, cx: &mut App) -> Vec<SavedWindow> {
    let set_aside = store.set_aside().map(|kept| kept.display().to_string());
    let restored = store.session().windows.clone();
    let store = Rc::new(RefCell::new(store));
    let save: flock_ui::session::Saver = Rc::new(move |session| {
        store
            .borrow_mut()
            .save(session)
            .map_err(|error| error.to_string())
    });
    flock_ui::session::install(save, cx);
    if let Some(kept) = set_aside {
        let notice = Toast::new(Status::Warn, TABS_SET_ASIDE).detail(format!("Kept as {kept}"));
        toast(notice, cx);
    }
    restored
}

fn window_options(cx: &App) -> WindowOptions {
    let window_size = size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT));
    let bounds = Bounds::centered(None, window_size, cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(TRAFFIC_LIGHT_X, TRAFFIC_LIGHT_Y)),
        }),
        window_min_size: Some(Size {
            width: px(MIN_WIDTH),
            height: px(MIN_HEIGHT),
        }),
        ..WindowOptions::default()
    }
}

fn open_startup_window(
    store: gpui::Entity<Store>,
    screen: Screen,
    preset: &Preset,
    cx: &mut App,
) -> anyhow::Result<()> {
    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
        let view = root_view(screen, preset, store.clone(), window, cx);
        if let Ok(shell) = view.clone().downcast::<Shell>() {
            subscribe_windows(&shell, store, cx);
        }
        cx.new(|cx| Root::new(view, window, cx))
    })
    .context("opening the Flock window")?;
    cx.activate(true);
    Ok(())
}

fn subscribe_windows(shell: &gpui::Entity<Shell>, store: gpui::Entity<Store>, cx: &mut App) {
    cx.subscribe(shell, move |_, request: &NewWindowRequest, cx| {
        if let Err(error) = open_flock_window(store.clone(), request.0.clone(), None, cx) {
            tracing::error!(%error, "opening a Flock window");
        }
    })
    .detach();
}

fn open_flock_window(
    store: gpui::Entity<Store>,
    initial: Vec<Command>,
    restored: Option<&SavedWindow>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let options = window_options(cx);
    cx.open_window(options, |window, cx| {
        let shell = cx.new(|cx| Shell::new(store.clone(), initial, window, cx));
        if let Some(saved) = restored {
            shell.update(cx, |shell, cx| shell.restore_tabs(saved, window, cx));
        }
        subscribe_windows(&shell, store, cx);
        cx.new(|cx| Root::new(shell, window, cx))
    })
    .context("opening the Flock window")?;
    cx.activate(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn bringing_flock_to_front_opens_in_the_front_window_or_a_new_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let (backend, updates) = async_channel::unbounded();
        let (commands, _received) = async_channel::unbounded();
        let snapshot = flock_demo::snapshot();
        let target = snapshot
            .home
            .problems
            .iter()
            .map(|problem| problem.target.clone())
            .find(|target| flock_core::FluxKind::from_label(&target.kind).is_none())
            .unwrap_or_else(|| panic!("the demo snapshot has no problems"));
        let store = cx.update(|cx| {
            flock_ui::init(Some(Appearance::Dark), cx)
                .unwrap_or_else(|error| panic!("UI init: {error}"));
            cx.new(|cx| Store::new(updates, commands, cx))
        });
        assert!(
            backend
                .try_send(flock_core::Update::Snapshot(Box::new(snapshot)))
                .is_ok()
        );
        cx.update(|cx| bring_to_front(store.clone(), None, cx));
        cx.run_until_parked();
        let first = cx
            .read(|cx| cx.windows().into_iter().next())
            .unwrap_or_else(|| panic!("no window opened"));
        let open = Command::OpenResource(target.clone());
        cx.update(|cx| bring_to_front(store.clone(), Some(open), cx));
        cx.run_until_parked();
        assert_eq!(cx.read(App::windows), vec![first]);
        let shown = cx.update(|cx| {
            first
                .downcast::<Root>()
                .and_then(|root| root.read(cx).ok().map(|root| root.view().clone()))
                .and_then(|view| view.downcast::<Shell>().ok())
                .map(|shell| shell.read(cx).state().location().resource.clone())
        });
        assert_eq!(shown, Some(Some(target)));
    }

    #[gpui::test]
    fn new_window_shortcut_and_close_keep_other_windows_open(cx: &mut gpui::TestAppContext) {
        let (backend, updates) = async_channel::unbounded();
        let (commands, _received) = async_channel::unbounded();
        cx.update(|cx| {
            launch(
                cx,
                Some(Appearance::Dark),
                Stores::default(),
                None,
                updates,
                commands,
                Screen::Flock,
                &Preset::default(),
            )
            .unwrap_or_else(|error| panic!("test launch: {error}"));
        });
        assert!(
            backend
                .try_send(flock_core::Update::Snapshot(Box::default()))
                .is_ok()
        );
        cx.run_until_parked();
        let first = cx
            .read(|cx| cx.windows().into_iter().next())
            .unwrap_or_else(|| panic!("first window missing"));
        cx.simulate_keystrokes(first, "cmd-n");
        let windows = cx.read(App::windows);
        assert_eq!(windows.len(), 2);
        let second = windows
            .into_iter()
            .find(|window| *window != first)
            .unwrap_or_else(|| panic!("second window missing"));
        cx.simulate_keystrokes(first, "cmd-shift-w");
        assert_eq!(cx.read(App::windows), vec![second]);
        cx.simulate_keystrokes(second, "cmd-n");
        assert_eq!(cx.read(|cx| cx.windows().len()), 2);
    }
}
