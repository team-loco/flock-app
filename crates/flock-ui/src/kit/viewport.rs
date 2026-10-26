use gpui::{
    Div, ElementId, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div,
};

pub fn scroll_viewport(id: impl Into<ElementId>, content: impl IntoElement) -> Div {
    let id = id.into();
    let selector = id.to_string();
    let scroll = div()
        .debug_selector(|| selector)
        .id(id)
        .absolute()
        .inset_0()
        .overflow_y_scroll()
        .child(content);
    div().relative().child(scroll)
}
