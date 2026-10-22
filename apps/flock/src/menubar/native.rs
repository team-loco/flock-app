#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSCellImagePosition, NSColor, NSFont, NSFontAttributeName,
    NSFontWeightRegular, NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    MainThreadMarker, NSAttributedString, NSAttributedStringKey, NSData, NSDictionary, NSObject,
    NSObjectProtocol, NSSize, NSString,
};

use super::Action;
use flock_core::{
    Sender,
    menubar::{ClusterItem, MenuBar},
};

const NO_CLUSTERS: &str = "No clusters";
const OPEN: &str = "Open Flock";
const OPEN_KEY: &str = "o";
const QUIT: &str = "Quit Flock";
const QUIT_KEY: &str = "q";
const NO_KEY: &str = "";
const ICON: &[u8] = include_bytes!("../../resources/MenuBarIcon.png");
const ICON_SIZE: NSSize = NSSize::new(18.0, 16.0);
const DEFAULT_FONT_SIZE: f64 = 0.0;

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
        if let Some(button) = item.button(mtm) {
            let data = NSData::with_bytes(ICON);
            let image = NSImage::initWithData(NSImage::alloc(), &data);
            if let Some(image) = &image {
                image.setSize(ICON_SIZE);
                image.setTemplate(true);
            }
            button.setImage(image.as_deref());
            button.setImagePosition(NSCellImagePosition::ImageLeft);
            let size = NSFont::menuBarFontOfSize(DEFAULT_FONT_SIZE).pointSize();
            let digits = NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe {
                NSFontWeightRegular
            });
            button.setFont(Some(&digits));
        }
        Self { item, mtm }
    }

    pub(super) fn update(&mut self, model: &MenuBar, sender: &Sender<Action>) {
        if let Some(button) = self.item.button(self.mtm) {
            button.setTitle(&NSString::from_str(&model.badge()));
            button.setAppearsDisabled(model.dimmed());
            let label = NSString::from_str(&model.accessibility_label());
            button.setAccessibilityLabel(Some(&label));
            button.setToolTip(Some(&label));
        }
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        if model.clusters.is_empty() {
            let empty = NSMenuItem::new(self.mtm);
            empty.setTitle(&NSString::from_str(NO_CLUSTERS));
            empty.setEnabled(false);
            menu.addItem(&empty);
        }
        for cluster in &model.clusters {
            let action = Action::Cluster(cluster.id.clone());
            let item = self.add(&menu, &cluster.name, NO_KEY, action, sender);
            let title = cluster_title(cluster);
            item.setAttributedTitle(Some(&title));
        }
        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        for (title, key, action) in [
            (OPEN, OPEN_KEY, Action::Open),
            (model.pause_title(), NO_KEY, Action::TogglePause),
            (QUIT, QUIT_KEY, Action::Quit),
        ] {
            self.add(&menu, title, key, action, sender);
        }
        self.item.setMenu(Some(&menu));
    }

    fn add(
        &self,
        menu: &NSMenu,
        title: &str,
        key: &str,
        action: Action,
        sender: &Sender<Action>,
    ) -> Retained<NSMenuItem> {
        let state = TargetState {
            sender: sender.clone(),
            action,
        };
        let allocated = Target::alloc(self.mtm).set_ivars(state);
        let target: Retained<Target> = unsafe { msg_send![super(allocated), init] };
        let item = NSMenuItem::new(self.mtm);
        item.setTitle(&NSString::from_str(title));
        item.setKeyEquivalent(&NSString::from_str(key));
        unsafe {
            item.setRepresentedObject(Some(&target));
            item.setTarget(Some(&target));
            item.setAction(Some(sel!(invoke:)));
        }
        menu.addItem(&item);
        item
    }
}

fn cluster_title(cluster: &ClusterItem) -> Retained<NSAttributedString> {
    let font = NSFont::menuFontOfSize(DEFAULT_FONT_SIZE);
    let disabled = (!cluster.available).then(NSColor::disabledControlTextColor);
    run(&cluster.name, &font, disabled.as_deref())
}

fn run(text: &str, font: &NSFont, colour: Option<&NSColor>) -> Retained<NSAttributedString> {
    let font_key: &NSAttributedStringKey = unsafe { NSFontAttributeName };
    let font_value: &AnyObject = font.as_ref();
    let mut keys = vec![font_key];
    let mut values = vec![font_value];
    if let Some(colour) = colour {
        keys.push(unsafe { NSForegroundColorAttributeName });
        values.push(colour.as_ref());
    }
    let attributes = NSDictionary::from_slices(&keys, &values);
    let string = NSString::from_str(text);
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &string,
            Some(&attributes),
        )
    }
}

impl Drop for Item {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
