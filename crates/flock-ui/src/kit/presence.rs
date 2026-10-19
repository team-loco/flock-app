use gpui::{
    Animation, AnimationExt, AnyElement, Context, ElementId, IntoElement, ParentElement, Pixels,
    SharedString, Styled, Task, div, ease_out_quint,
};

use crate::theme::{INSPECTOR_IN, INSPECTOR_OUT, INSPECTOR_SLIDE, INSPECTOR_SWAP};

const ENTER: &str = "enter";
const LEAVE: &str = "leave";
const SWAP: &str = "swap";
const LEAVING: u64 = 0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Motion {
    #[default]
    Still,
    Enter(u64),
    Swap(u64),
}

#[derive(Default)]
pub struct Entrance {
    key: Option<String>,
    motion: Motion,
    generation: u64,
    settle: Option<Task<()>>,
}

impl Entrance {
    pub fn track<V: 'static>(
        &mut self,
        key: Option<&str>,
        field: impl Fn(&mut V) -> &mut Self + 'static,
        cx: &mut Context<V>,
    ) -> Motion {
        let previous = std::mem::replace(&mut self.key, key.map(str::to_owned));
        let Some(key) = key else {
            self.motion = Motion::Still;
            self.settle = None;
            return self.motion;
        };
        let entering = match previous.as_deref() {
            None => true,
            Some(previous) if previous != key => false,
            Some(_) => return self.motion,
        };
        if cx.reduce_motion() {
            self.motion = Motion::Still;
            self.settle = None;
            return self.motion;
        }
        self.generation += 1;
        let (motion, duration) = if entering {
            (Motion::Enter(self.generation), INSPECTOR_IN)
        } else {
            (Motion::Swap(self.generation), INSPECTOR_SWAP)
        };
        self.motion = motion;
        self.settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |view, cx| {
                field(view).motion = Motion::Still;
                cx.notify();
            })
            .ok();
        }));
        motion
    }
}

pub struct Presence<T> {
    shown: Option<T>,
    leaving: Option<T>,
    exit: Option<Task<()>>,
    entrance: Entrance,
}

impl<T> Default for Presence<T> {
    fn default() -> Self {
        Self {
            shown: None,
            leaving: None,
            exit: None,
            entrance: Entrance::default(),
        }
    }
}

impl<T: Clone + 'static> Presence<T> {
    pub fn track<V: 'static>(
        &mut self,
        current: Option<(&T, &str)>,
        field: fn(&mut V) -> &mut Self,
        cx: &mut Context<V>,
    ) -> Motion {
        let key = current.map(|(_, key)| key);
        let motion = self
            .entrance
            .track(key, move |view| &mut field(view).entrance, cx);
        let current = current.map(|(shown, _)| shown);
        let previous = std::mem::replace(&mut self.shown, current.cloned());
        if current.is_some() {
            self.leaving = None;
            self.exit = None;
            return motion;
        }
        let Some(previous) = previous else {
            return motion;
        };
        if cx.reduce_motion() {
            return motion;
        }
        self.leaving = Some(previous);
        self.exit = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INSPECTOR_OUT).await;
            this.update(cx, |view, cx| {
                field(view).leaving = None;
                cx.notify();
            })
            .ok();
        }));
        motion
    }

    pub fn leaving(&self) -> Option<&T> {
        self.leaving.as_ref()
    }
}

fn key(id: &str, phase: &str, generation: u64) -> ElementId {
    ElementId::Name(SharedString::from(format!("{id}-{phase}-{generation}")))
}

pub fn swap_in(element: impl IntoElement, id: &str, motion: Motion) -> AnyElement {
    let content = div().relative().size_full().child(element);
    let generation = match motion {
        Motion::Still => return content.into_any_element(),
        Motion::Enter(generation) => generation,
        Motion::Swap(generation) => generation,
    };
    let animation = Animation::new(INSPECTOR_SWAP).with_easing(ease_out_quint());
    content
        .with_animation(key(id, SWAP, generation), animation, |content, delta| {
            content.opacity(delta).left(INSPECTOR_SLIDE * (1. - delta))
        })
        .into_any_element()
}

#[derive(gpui::IntoElement)]
struct Slide {
    element: AnyElement,
    id: String,
    width: Pixels,
    motion: Option<Motion>,
}

impl gpui::RenderOnce for Slide {
    fn render(self, _: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let width = super::inspector::stored_width(&self.id, self.width, cx);
        let content = match self.motion {
            Some(motion) => swap_in(self.element, &self.id, motion),
            None => self.element,
        };
        let panel = div()
            .flex_none()
            .h_full()
            .overflow_hidden()
            .w(width)
            .child(div().flex_none().w(width).h_full().child(content));
        let (phase, generation, duration, entering) = match self.motion {
            Some(Motion::Enter(generation)) => (ENTER, generation, INSPECTOR_IN, true),
            Some(Motion::Swap(_)) => return panel.into_any_element(),
            Some(Motion::Still) => return panel.into_any_element(),
            None => (LEAVE, LEAVING, INSPECTOR_OUT, false),
        };
        let animation = Animation::new(duration).with_easing(ease_out_quint());
        panel
            .with_animation(
                key(&self.id, phase, generation),
                animation,
                move |panel, delta| {
                    let shown = if entering { delta } else { 1. - delta };
                    panel.w(width * shown).opacity(shown)
                },
            )
            .into_any_element()
    }
}

pub fn slide_in(element: impl IntoElement, id: &str, motion: Motion, width: Pixels) -> AnyElement {
    Slide {
        element: element.into_any_element(),
        id: id.to_owned(),
        width,
        motion: Some(motion),
    }
    .into_any_element()
}

pub fn slide_out(element: impl IntoElement, id: &str, width: Pixels) -> AnyElement {
    Slide {
        element: element.into_any_element(),
        id: id.to_owned(),
        width,
        motion: None,
    }
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    #[derive(Default)]
    struct View {
        presence: Presence<&'static str>,
    }

    fn presence(view: &mut View) -> &mut Presence<&'static str> {
        &mut view.presence
    }

    fn track(
        view: &gpui::Entity<View>,
        current: Option<&'static str>,
        cx: &mut TestAppContext,
    ) -> Motion {
        view.update(cx, |view, cx| {
            let shown = current.as_ref().map(|shown| (shown, *shown));
            view.presence.track(shown, presence, cx)
        })
    }

    fn leaving(view: &gpui::Entity<View>, cx: &mut TestAppContext) -> Option<&'static str> {
        view.read_with(cx, |view, _| view.presence.leaving().copied())
    }

    #[gpui::test]
    fn a_closed_inspector_lingers_until_its_exit_finishes(cx: &mut TestAppContext) {
        let view = cx.new(|_| View::default());
        track(&view, Some("a"), cx);
        track(&view, Some("b"), cx);
        assert_eq!(leaving(&view, cx), None);
        track(&view, None, cx);
        assert_eq!(leaving(&view, cx), Some("b"));
        track(&view, None, cx);
        assert_eq!(leaving(&view, cx), Some("b"));
        cx.executor().advance_clock(INSPECTOR_OUT);
        cx.run_until_parked();
        assert_eq!(leaving(&view, cx), None);
    }

    #[gpui::test]
    fn selecting_again_cancels_the_exit(cx: &mut TestAppContext) {
        let view = cx.new(|_| View::default());
        track(&view, Some("a"), cx);
        track(&view, None, cx);
        track(&view, Some("c"), cx);
        assert_eq!(leaving(&view, cx), None);
        cx.executor().advance_clock(INSPECTOR_OUT);
        cx.run_until_parked();
        track(&view, Some("c"), cx);
        assert_eq!(leaving(&view, cx), None);
    }

    #[gpui::test]
    fn only_a_new_inspector_or_selection_moves_and_then_comes_to_rest(cx: &mut TestAppContext) {
        let view = cx.new(|_| View::default());
        assert_eq!(track(&view, Some("a"), cx), Motion::Enter(1));
        assert_eq!(track(&view, Some("a"), cx), Motion::Enter(1));
        cx.executor().advance_clock(INSPECTOR_IN);
        cx.run_until_parked();
        assert_eq!(track(&view, Some("a"), cx), Motion::Still);
        assert_eq!(track(&view, Some("b"), cx), Motion::Swap(2));
        cx.executor().advance_clock(INSPECTOR_SWAP);
        cx.run_until_parked();
        assert_eq!(track(&view, Some("b"), cx), Motion::Still);
        assert_eq!(track(&view, None, cx), Motion::Still);
        assert_eq!(track(&view, Some("b"), cx), Motion::Enter(3));
    }

    #[gpui::test]
    fn reduced_motion_never_moves_the_inspector(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let view = cx.new(|_| View::default());
        assert_eq!(track(&view, Some("a"), cx), Motion::Still);
        assert_eq!(track(&view, Some("b"), cx), Motion::Still);
    }

    #[gpui::test]
    fn reduced_motion_removes_the_inspector_at_once(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let view = cx.new(|_| View::default());
        track(&view, Some("a"), cx);
        track(&view, None, cx);
        assert_eq!(leaving(&view, cx), None);
    }
}
