use gpui::{Entity, Render, StyleRefinement, Styled, ViewElement};

pub fn pane<V: Render>(view: &Entity<V>) -> ViewElement<Entity<V>> {
    view.clone().cached(StyleRefinement::default().size_full())
}
