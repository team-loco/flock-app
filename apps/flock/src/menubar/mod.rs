mod native;

use flock_core::{ClusterId, Command, Sender, Snapshot, Update, menubar::MenuBar};
use flock_ui::{Store, settings};
use gpui::{App, AppContext as _, Context, Entity, Global, Subscription, Task};
use objc2_foundation::MainThreadMarker;

#[derive(Clone)]
enum Action {
    Cluster(ClusterId),
    Open,
    TogglePause,
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
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::error!("the menu bar item can only be created on the main thread");
        return;
    };
    let updates = store.update(cx, |store, _| store.subscribe());
    let state = cx.new(|cx: &mut Context<MenuBarState>| {
        let (sender, actions) = async_channel::unbounded();
        let settings = cx.observe_global::<settings::Settings>(|this, cx| this.refresh(cx));
        let updates = cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let Update::Snapshot(snapshot) = update else {
                    continue;
                };
                let result = this.update(cx, |this, cx| {
                    let model = MenuBar::new(&snapshot, paused(cx));
                    if this.model != model {
                        this.model = model;
                        this.redraw();
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        });
        let actions = cx.spawn(async move |_, cx| {
            while let Ok(action) = actions.recv().await {
                cx.update(|cx| run(action, store.clone(), cx));
            }
        });
        let mut state = MenuBarState {
            item: None,
            model: MenuBar::new(&Snapshot::default(), paused(cx)),
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

fn run(action: Action, store: Entity<Store>, cx: &mut App) {
    match action {
        Action::Quit => cx.quit(),
        Action::Open => super::bring_to_front(store, None, cx),
        Action::TogglePause => {
            let result = settings::update(cx, |s| {
                s.notifications.paused = !s.notifications.paused;
            });
            if let Err(error) = result {
                tracing::warn!(%error, "saving the notification pause");
            }
        }
        Action::Cluster(id) => {
            let select = Command::SelectCluster(Some(id));
            super::bring_to_front(store, Some(select), cx);
        }
    }
}

fn paused(cx: &App) -> bool {
    settings::current(cx).notifications.paused
}

impl MenuBarState {
    fn refresh(&mut self, cx: &mut App) {
        let wanted = settings::current(cx).keeps_running_in_menu_bar();
        if wanted && self.item.is_none() {
            self.item = Some(native::Item::new(self.mtm));
            self.redraw();
        }
        if !wanted {
            self.item = None;
        }
        let paused = paused(cx);
        if self.model.paused != paused {
            self.model.paused = paused;
            self.redraw();
        }
    }

    fn redraw(&mut self) {
        if let Some(item) = &mut self.item {
            item.update(&self.model, &self.sender);
        }
    }
}
