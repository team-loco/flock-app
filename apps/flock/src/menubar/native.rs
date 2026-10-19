#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};

use super::Action;
use flock_core::{Sender, menubar::MenuBar};

struct TargetState {
    sender: Sender<Action>,
    action: Action,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "FlockMenuBarTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(invoke:))]
        fn invoke(&self, _: &NSMenuItem) {
            let state = self.ivars();
            let _ = state.sender.try_send(state.action.clone());
        }
    }
);

pub(super) struct Item {
    item: Retained<NSStatusItem>,
    mtm: MainThreadMarker,
}

impl Item {
    pub(super) fn new(mtm: MainThreadMarker) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        Self { item, mtm }
    }

    pub(super) fn update(&mut self, model: &MenuBar, sender: &Sender<Action>) {
        if let Some(button) = self.item.button(self.mtm) {
            button.setTitle(&NSString::from_str(&model.label));
            button.setToolTip(Some(&NSString::from_str("Flock fleet health")));
        }
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        if model.problems.is_empty() {
            let empty = NSMenuItem::new(self.mtm);
            empty.setTitle(&NSString::from_str("No problems"));
            empty.setEnabled(false);
            menu.addItem(&empty);
        }
        for problem in &model.problems {
            let action = Action::Problem(problem.target.clone());
            self.add(&menu, &problem.title, action, sender);
        }
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        for (title, action) in [
            ("Open Flock", Action::Open),
            ("Settings…", Action::Settings),
            ("Quit", Action::Quit),
        ] {
            self.add(&menu, title, action, sender);
        }
        self.item.setMenu(Some(&menu));
    }

    fn add(&self, menu: &NSMenu, title: &str, action: Action, sender: &Sender<Action>) {
        let state = TargetState {
            sender: sender.clone(),
            action,
        };
        let allocated = Target::alloc(self.mtm).set_ivars(state);
        let target: Retained<Target> = unsafe { msg_send![super(allocated), init] };
        let item = NSMenuItem::new(self.mtm);
        item.setTitle(&NSString::from_str(title));
        unsafe {
            item.setRepresentedObject(Some(&target));
            item.setTarget(Some(&target));
            item.setAction(Some(sel!(invoke:)));
        }
        menu.addItem(&item);
    }
}

impl Drop for Item {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
