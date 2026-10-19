mod native;

use flock_core::{Command, Page, ResourceRef, Sender, Snapshot, Update, menubar::MenuBar};
use flock_ui::{Store, settings};
use gpui::{App, AppContext as _, Context, Entity, Global, Subscription, Task};
use objc2_foundation::MainThreadMarker;

#[derive(Clone)]
enum Action {
    Problem(ResourceRef),
    Open,
    Settings,
    Quit,
}

struct MenuBarState {
    item: Option<native::Item>,
    model: MenuBar,
    sender: Sender<Action>,
    mtm: MainThreadMarker,
    _settings: Subscription,
    _updates: Task<()>,
    _actions: Task<()>,
}

struct Owner {
    _state: Entity<MenuBarState>,
}

impl Global for Owner {}

pub(super) fn install(store: Entity<Store>, cx: &mut App) {
    let mtm = MainThreadMarker::new()
        .unwrap_or_else(|| panic!("menu bar must be installed on the main thread"));
    let updates = store.update(cx, |store, _| store.subscribe());
    let state = cx.new(|cx: &mut Context<MenuBarState>| {
        let (sender, actions) = async_channel::unbounded();
        let settings = cx.observe_global::<settings::Settings>(|this, cx| {
            this.refresh(cx);
            if cx.windows().is_empty() && !settings::current(cx).show_in_menu_bar {
                cx.quit();
            }
        });
        let updates = cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if let Update::Snapshot(snapshot) = update {
                    let result = this.update(cx, |this, cx| {
                        let model = MenuBar::from_snapshot(&snapshot);
                        if this.model != model {
                            this.model = model;
                            this.refresh(cx);
                        }
                    });
                    if result.is_err() {
                        break;
                    }
                }
            }
        });
        let actions = cx.spawn(async move |_, cx| {
            while let Ok(action) = actions.recv().await {
                cx.update(|cx| match action {
                    Action::Quit => cx.quit(),
                    Action::Open => open(store.clone(), Vec::new(), cx),
                    Action::Settings => {
                        open(store.clone(), vec![Command::SelectPage(Page::Settings)], cx);
                    }
                    Action::Problem(target) => {
                        open(store.clone(), vec![Command::OpenResource(target)], cx);
                    }
                });
            }
        });
        let mut state = MenuBarState {
            item: None,
            model: MenuBar::from_snapshot(&Snapshot::default()),
            sender,
            mtm,
            _settings: settings,
            _updates: updates,
            _actions: actions,
        };
        state.refresh(cx);
        state
    });
    cx.set_global(Owner { _state: state });
}

impl MenuBarState {
    fn refresh(&mut self, cx: &mut App) {
        if settings::current(cx).show_in_menu_bar {
            let item = self.item.get_or_insert_with(|| native::Item::new(self.mtm));
            item.update(&self.model, &self.sender);
        } else {
            self.item.take();
        }
    }
}

fn open(store: Entity<Store>, initial: Vec<Command>, cx: &mut App) {
    if initial.is_empty()
        && let Some(window) = cx.windows().first()
    {
        let _ = window.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
        return;
    }
    if let Err(error) = super::open_flock_window(store, initial, None, cx) {
        tracing::error!(%error, "opening Flock from the menu bar");
    }
}
