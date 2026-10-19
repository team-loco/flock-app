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
                let model = MenuBar::from_snapshot(&snapshot);
                let result = this.update(cx, |this, _| {
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

fn run(action: Action, store: Entity<Store>, cx: &mut App) {
    match action {
        Action::Quit => cx.quit(),
        Action::Open => super::bring_to_front(store, None, cx),
        Action::Settings => {
            let settings = Command::SelectPage(Page::Settings);
            super::bring_to_front(store, Some(settings), cx);
        }
        Action::Problem(target) => {
            super::bring_to_front(store, Some(Command::OpenResource(target)), cx);
        }
    }
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
    }

    fn redraw(&mut self) {
        if let Some(item) = &mut self.item {
            item.update(&self.model, &self.sender);
        }
    }
}
