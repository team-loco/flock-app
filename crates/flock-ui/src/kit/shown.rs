use gpui::Context;

pub struct Shown<T>(Option<T>);

impl<T> Default for Shown<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: PartialEq> Shown<T> {
    pub fn show<V: 'static>(&mut self, content: T, cx: &mut Context<V>) {
        if self.0.as_ref() == Some(&content) {
            return;
        }
        self.0 = Some(content);
        cx.notify();
    }

    pub fn get(&self) -> Option<&T> {
        self.0.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    #[gpui::test]
    fn showing_the_same_content_does_not_notify(cx: &mut TestAppContext) {
        let view = cx.new(|_| Shown::<u8>::default());
        let notified = Rc::new(Cell::new(0));
        let count = Rc::clone(&notified);
        let _observer = cx.update(|cx| cx.observe(&view, move |_, _| count.set(count.get() + 1)));
        view.update(cx, |shown, cx| shown.show(1, cx));
        view.update(cx, |shown, cx| shown.show(1, cx));
        assert_eq!(notified.get(), 1);
        view.update(cx, |shown, cx| shown.show(2, cx));
        assert_eq!(notified.get(), 2);
        assert_eq!(view.read_with(cx, |shown, _| shown.get().copied()), Some(2));
    }
}
