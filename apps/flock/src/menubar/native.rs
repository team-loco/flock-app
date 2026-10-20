#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSCellImagePosition, NSColor, NSFont, NSFontAttributeName, NSFontWeightRegular,
    NSForegroundColorAttributeName, NSImage, NSMenu, NSMenuItem, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSStatusBar, NSStatusItem, NSTextAlignment, NSTextTab,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSData, NSDictionary,
    NSMutableAttributedString, NSObject, NSObjectProtocol, NSSize, NSString,
};

use super::Action;
use flock_core::{Sender, Status, menubar::MenuBar};

const TOOLTIP: &str = "Flock cluster health";
const NO_CLUSTERS: &str = "No clusters";
const OPEN: &str = "Open Flock";
const OPEN_KEY: &str = "o";
const QUIT: &str = "Quit Flock";
const QUIT_KEY: &str = "q";
const NO_KEY: &str = "";
const ICON: &[u8] = include_bytes!("../../resources/MenuBarIcon.png");
const ICON_SIZE: NSSize = NSSize::new(18.0, 16.0);
const GLYPH_FONT_SIZE: f64 = 10.0;
const META_FONT_SIZE: f64 = 11.0;
const MENU_FONT_SIZE: f64 = 0.0;
const META_TAB: f64 = 240.0;
const GLYPH_GAP: &str = "  ";
const TAB: &str = "\t";

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
            button.setToolTip(Some(&NSString::from_str(TOOLTIP)));
        }
        Self { item, mtm }
    }

    pub(super) fn update(&mut self, model: &MenuBar, sender: &Sender<Action>) {
        if let Some(button) = self.item.button(self.mtm) {
            button.setTitle(&NSString::from_str(&model.badge));
            button.setAppearsDisabled(model.dimmed);
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

fn cluster_title(cluster: &flock_core::menubar::ClusterItem) -> Retained<NSAttributedString> {
    let title = NSMutableAttributedString::new();
    let mono =
        NSFont::monospacedSystemFontOfSize_weight(GLYPH_FONT_SIZE, unsafe { NSFontWeightRegular });
    let meta_font =
        NSFont::monospacedSystemFontOfSize_weight(META_FONT_SIZE, unsafe { NSFontWeightRegular });
    let menu_font = NSFont::menuFontOfSize(MENU_FONT_SIZE);
    let glyph = run(
        cluster.status.glyph(),
        &mono,
        Some(&status_colour(cluster.status)),
    );
    title.appendAttributedString(&glyph);
    let gap = run(GLYPH_GAP, &menu_font, None);
    title.appendAttributedString(&gap);
    let name_colour = (cluster.status == Status::Offline).then(NSColor::secondaryLabelColor);
    let name = run(&cluster.name, &menu_font, name_colour.as_deref());
    title.appendAttributedString(&name);
    if !cluster.meta.is_empty() {
        let tab = run(TAB, &menu_font, None);
        title.appendAttributedString(&tab);
        let colour = status_colour(cluster.meta_status);
        let meta = run(&cluster.meta, &meta_font, Some(&colour));
        title.appendAttributedString(&meta);
    }
    let style = NSMutableParagraphStyle::new();
    let options = NSDictionary::new();
    let stop = unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Right,
            META_TAB,
            &options,
        )
    };
    let stops = NSArray::from_retained_slice(&[stop]);
    style.setTabStops(Some(&stops));
    let length = title.length();
    let whole = objc2_foundation::NSRange::new(0, length);
    let key: &NSAttributedStringKey = unsafe { NSParagraphStyleAttributeName };
    let value: &AnyObject = style.as_ref();
    unsafe { title.addAttribute_value_range(key, value, whole) };
    Retained::into_super(title)
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

fn status_colour(status: Status) -> Retained<NSColor> {
    match status {
        Status::Ok => NSColor::systemGreenColor(),
        Status::Warn => NSColor::systemOrangeColor(),
        Status::Err => NSColor::systemRedColor(),
        Status::Progress => NSColor::systemBlueColor(),
        Status::Suspended => NSColor::secondaryLabelColor(),
        Status::Offline => NSColor::secondaryLabelColor(),
    }
}

impl Drop for Item {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
