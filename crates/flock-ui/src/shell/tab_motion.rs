use std::collections::{HashMap, HashSet};

use flock_core::tabs::{TabId, TabTitle};
use gpui::{
    Animation, AnimationExt, AnyElement, ElementId, InteractiveElement, IntoElement, ParentElement,
    Pixels, Styled, Task, div, ease_out_quint, prelude::FluentBuilder,
};

use crate::theme::{TAB_ENTER, TAB_GROW_MAX, TAB_LEAVE, TAB_SETTLE, TAB_SETTLE_SLIDE};

#[derive(Clone)]
pub(super) struct Ghost {
    pub(super) before: usize,
    pub(super) id: TabId,
    pub(super) title: TabTitle,
}

#[derive(Clone, Copy)]
pub(super) struct Settling {
    generation: u64,
    from_left: bool,
}

#[derive(Default)]
pub(super) struct StripMotion {
    known: Option<Vec<(TabId, TabTitle)>>,
    entered: HashSet<TabId>,
    settling: HashMap<TabId, Settling>,
    generation: u64,
    pub(super) ghosts: Vec<Ghost>,
    pub(super) timer: Option<Task<()>>,
}

impl StripMotion {
    pub(super) fn track(&mut self, current: &[(TabId, TabTitle)], animate: bool) -> bool {
        let previous = self.known.replace(current.to_vec());
        let live: HashSet<TabId> = current.iter().map(|(id, _)| *id).collect();
        self.entered.retain(|id| live.contains(id));
        self.settling.retain(|id, _| live.contains(id));
        let Some(previous) = previous else {
            return false;
        };
        if !animate {
            self.entered.clear();
            self.settling.clear();
            self.ghosts.clear();
            return false;
        }
        let before: HashSet<TabId> = previous.iter().map(|(id, _)| *id).collect();
        let mut changed = false;
        for (id, _) in current {
            if !before.contains(id) {
                self.entered.insert(*id);
                changed = true;
            }
        }
        for (ix, (id, title)) in previous.iter().enumerate() {
            if live.contains(id) {
                continue;
            }
            let kept_before = previous[..ix]
                .iter()
                .filter(|(other, _)| live.contains(other))
                .count();
            self.ghosts.push(Ghost {
                before: kept_before,
                id: *id,
                title: title.clone(),
            });
            self.entered.remove(id);
            changed = true;
        }
        if let Some((id, from_left)) = moved(&previous, current) {
            self.generation += 1;
            self.settling.insert(
                id,
                Settling {
                    generation: self.generation,
                    from_left,
                },
            );
            changed = true;
        }
        changed
    }

    pub(super) fn finish(&mut self) {
        self.ghosts.clear();
        self.entered.clear();
        self.settling.clear();
    }

    pub(super) fn child_index(&self, ix: usize) -> usize {
        let ghosts = self
            .ghosts
            .iter()
            .filter(|ghost| ghost.before <= ix)
            .count();
        ix + ghosts
    }

    pub(super) fn wrap(&self, id: TabId, slot: String, tab: AnyElement) -> AnyElement {
        let mut element = tab;
        if let Some(settling) = self.settling.get(&id) {
            element = settle(id, *settling, element);
        }
        if self.entered.contains(&id) {
            element = grow(id, slot, element);
        }
        element
    }
}

fn moved(previous: &[(TabId, TabTitle)], current: &[(TabId, TabTitle)]) -> Option<(TabId, bool)> {
    let current_ids: HashSet<TabId> = current.iter().map(|(id, _)| *id).collect();
    let previous_ids: HashSet<TabId> = previous.iter().map(|(id, _)| *id).collect();
    let old: Vec<TabId> = previous
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| current_ids.contains(id))
        .collect();
    let new: Vec<TabId> = current
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| previous_ids.contains(id))
        .collect();
    old.iter()
        .enumerate()
        .filter_map(|(old_ix, id)| {
            let new_ix = new.iter().position(|other| other == id)?;
            (new_ix != old_ix).then_some((*id, old_ix.abs_diff(new_ix), old_ix < new_ix))
        })
        .max_by_key(|(_, distance, _)| *distance)
        .map(|(id, _, to_right)| (id, to_right))
}

fn grow(id: TabId, slot: String, tab: AnyElement) -> AnyElement {
    let animation = Animation::new(TAB_ENTER).with_easing(ease_out_quint());
    div()
        .debug_selector(move || slot.clone())
        .flex()
        .flex_none()
        .h_full()
        .overflow_hidden()
        .child(tab)
        .with_animation(
            ElementId::NamedInteger("tab-enter".into(), id.as_u64()),
            animation,
            clamp,
        )
        .into_any_element()
}

fn clamp<E: Styled + InteractiveElement + IntoElement>(element: E, delta: f32) -> E {
    element.opacity(delta).when(delta < 1., |element| {
        let width: Pixels = TAB_GROW_MAX * delta;
        element.max_w(width)
    })
}

pub(super) fn ghost(ghost: &Ghost, slot: String, content: AnyElement) -> AnyElement {
    let animation = Animation::new(TAB_LEAVE).with_easing(ease_out_quint());
    div()
        .debug_selector(move || slot.clone())
        .flex()
        .flex_none()
        .h_full()
        .overflow_hidden()
        .child(content)
        .with_animation(
            ElementId::NamedInteger("tab-leave".into(), ghost.id.as_u64()),
            animation,
            |element, delta| clamp(element, 1. - delta),
        )
        .into_any_element()
}

fn settle(id: TabId, settling: Settling, tab: AnyElement) -> AnyElement {
    let animation = Animation::new(TAB_SETTLE).with_easing(ease_out_quint());
    let sign = if settling.from_left { -1. } else { 1. };
    div()
        .relative()
        .flex()
        .flex_none()
        .h_full()
        .child(tab)
        .with_animation(
            ElementId::Name(format!("tab-settle-{}-{}", id.as_u64(), settling.generation).into()),
            animation,
            move |element, delta| {
                element
                    .opacity(0.5 + 0.5 * delta)
                    .left(TAB_SETTLE_SLIDE * (sign * (1. - delta)))
            },
        )
        .into_any_element()
}
