use std::rc::Rc;

use flock_core::ResourceRef;
use flock_core::events::Event;
use flock_core::pods::PodEvent;
use flock_core::time::age;
use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
};

use crate::kit::{
    EventList, IconButton, Inspector, KeyValue, Link, Presence, RawMessage, SelectableText,
    StatusText, slide_in, slide_out,
};
use crate::theme::INSPECTOR;

pub const INSPECTOR_ID: &str = "event-inspector";
const MESSAGE_SELECTOR: &str = "event-message";
const NO_OTHER_EVENTS: &str = "No other events for this object.";

pub type Close = Rc<dyn Fn(&mut Window, &mut App)>;
pub type OpenObject = Rc<dyn Fn(ResourceRef, &mut Window, &mut App)>;

pub struct EventInspector<'a> {
    pub event: &'a Event,
    pub events: &'a [Event],
    pub target: Option<ResourceRef>,
    pub close: Close,
    pub open: OpenObject,
}

impl EventInspector<'_> {
    pub fn into_element(self) -> AnyElement {
        let event = self.event;
        let close = self.close;
        let close = IconButton::glyph("close-event", "×", "Close")
            .on_click(move |_, window, cx| close(window, cx));
        let details = &event.details;
        let first_seen = seen(details.first_age, details.first_seen.as_deref());
        let last_seen = seen(Some(event.age), details.last_seen.as_deref());
        let facts = KeyValue::new()
            .sans()
            .text("Reason", event.reason.clone())
            .row("Type", StatusText::new(event.status, event_type(event)))
            .text("Count", event.count.to_string())
            .text("First seen", first_seen)
            .text("Last seen", last_seen)
            .text("Component", value(&details.source_component))
            .text("Host", value(&details.source_host));
        let object = match self.target {
            Some(target) => {
                let open = self.open;
                Link::new("event-object", event.object.clone())
                    .mono()
                    .on_click(move |_, window, cx| open(target.clone(), window, cx))
                    .into_any_element()
            }
            None => SelectableText::new(value(&event.object)).into_any_element(),
        };
        let others: Vec<PodEvent> = self
            .events
            .iter()
            .filter(|other| event.same_object(other) && event.key() != other.key())
            .map(PodEvent::from)
            .collect();
        let related = if others.is_empty() {
            div().child(NO_OTHER_EVENTS).into_any_element()
        } else {
            EventList::new(others).into_any_element()
        };
        Inspector::new(INSPECTOR_ID, event.reason.clone())
            .status(event.status, event_type(event))
            .actions(close)
            .section(
                Some("Message"),
                div()
                    .debug_selector(|| MESSAGE_SELECTOR.to_owned())
                    .min_w_0()
                    .child(RawMessage::new(event.message.clone())),
            )
            .section(Some("Details"), facts)
            .section(Some("Involved object"), object)
            .section(Some("Other events"), related)
            .into_any_element()
    }
}

pub fn slot<V: 'static>(
    current: Option<Event>,
    presence: &mut Presence<Event>,
    field: fn(&mut V) -> &mut Presence<Event>,
    build: impl Fn(&Event, &mut Context<V>) -> AnyElement,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let selection = current.as_ref().map(Event::key);
    let shown = current.as_ref().zip(selection.as_deref());
    let motion = presence.track(shown, field, cx);
    if let Some(event) = current {
        return Some(slide_in(build(&event, cx), INSPECTOR_ID, motion, INSPECTOR));
    }
    let leaving = presence.leaving().cloned()?;
    Some(slide_out(build(&leaving, cx), INSPECTOR_ID, INSPECTOR))
}

fn value(text: &str) -> String {
    if text.is_empty() {
        "Unknown".into()
    } else {
        text.into()
    }
}

fn event_type(event: &Event) -> String {
    if !event.details.event_type.is_empty() {
        return event.details.event_type.clone();
    }
    if event.is_warning() {
        "Warning".into()
    } else {
        "Normal".into()
    }
}

fn seen(duration: Option<std::time::Duration>, absolute: Option<&str>) -> String {
    match (duration, absolute) {
        (Some(duration), Some(absolute)) => format!("{} ago · {absolute}", age(duration)),
        _ => "Unknown".into(),
    }
}
