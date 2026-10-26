use gpui::{
    AnyElement, Div, ElementId, InteractiveElement, Interactivity, IntoElement, ParentElement,
    ScrollHandle, StatefulInteractiveElement, StyleRefinement, Styled, div,
};

pub struct ScrollViewport {
    slot: Div,
    id: ElementId,
    content: AnyElement,
    both_axes: bool,
    handle: Option<ScrollHandle>,
}

pub fn scroll_viewport(id: impl Into<ElementId>, content: impl IntoElement) -> ScrollViewport {
    ScrollViewport {
        slot: div().relative(),
        id: id.into(),
        content: content.into_any_element(),
        both_axes: false,
        handle: None,
    }
}

impl ScrollViewport {
    pub fn both_axes(mut self) -> Self {
        self.both_axes = true;
        self
    }

    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.handle = Some(handle.clone());
        self
    }
}

impl Styled for ScrollViewport {
    fn style(&mut self) -> &mut StyleRefinement {
        self.slot.style()
    }
}

impl InteractiveElement for ScrollViewport {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.slot.interactivity()
    }
}

impl IntoElement for ScrollViewport {
    type Element = Div;

    fn into_element(self) -> Div {
        let selector = self.id.to_string();
        let scroll = div()
            .debug_selector(|| selector)
            .id(self.id)
            .absolute()
            .inset_0();
        let scroll = if self.both_axes {
            scroll.overflow_scroll()
        } else {
            scroll.overflow_y_scroll()
        };
        let scroll = match &self.handle {
            Some(handle) => scroll.track_scroll(handle),
            None => scroll,
        };
        self.slot.child(scroll.child(self.content))
    }
}
