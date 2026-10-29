use std::borrow::Cow;
use std::ops::Deref;
use std::rc::Rc;

use flock_core::Status;
use flock_core::filter::Searchable;
use flock_core::grouping::Group;
use gpui::{
    Div, FontWeight, InteractiveElement, ParentElement, Pixels, SharedString, Stateful, Styled,
    div, prelude::FluentBuilder, svg,
};

use super::status::status_symbols;
use crate::assets::{CHEVRON_DOWN, CHEVRON_RIGHT};
use crate::theme::{FONT_MONO, ICON, Palette, RULE, SPACE_4, TEXT_AXIS, TEXT_MONO, TEXT_SMALL};

pub const GROUP_PREFIX: &str = "group";
pub const UNGROUPED: &str = "None";

pub type GroupKey<R> = Rc<dyn Fn(&R) -> Option<Cow<'_, str>>>;

pub struct GroupOption<R> {
    pub facet: SharedString,
    pub key: GroupKey<R>,
}

pub struct Grouping<R> {
    pub options: Vec<GroupOption<R>>,
    pub status: Rc<dyn Fn(&R) -> Status>,
}

impl<R: 'static> Grouping<R> {
    pub fn new(status: impl Fn(&R) -> Status + 'static) -> Self {
        Self {
            options: Vec::new(),
            status: Rc::new(status),
        }
    }

    pub fn by(
        mut self,
        facet: impl Into<SharedString>,
        key: impl Fn(&R) -> Option<Cow<'_, str>> + 'static,
    ) -> Self {
        self.options.push(GroupOption {
            facet: facet.into(),
            key: Rc::new(key),
        });
        self
    }

    pub fn facets<T>(status: impl Fn(&R) -> Status + 'static) -> Self
    where
        R: Deref<Target = T>,
        T: Searchable,
    {
        T::FILTER
            .group_facets()
            .into_iter()
            .fold(Self::new(status), |grouping, facet| {
                grouping.by(facet, move |row: &R| {
                    let row: &T = row;
                    T::FILTER.group_value(row, &(), row.namespace(), facet)
                })
            })
    }
}

pub fn option_label(facet: &str) -> SharedString {
    let mut chars = facet.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
    format!("{}{}", first.unwrap_or_default(), chars.as_str()).into()
}

pub struct Header<'a> {
    pub id: SharedString,
    pub facet: &'a str,
    pub group: &'a Group,
    pub focused: bool,
    pub height: Pixels,
    pub inset: Pixels,
    pub trailing: Pixels,
}

impl Header<'_> {
    pub fn render(self, palette: &Palette) -> Stateful<Div> {
        let chevron = if self.group.collapsed {
            CHEVRON_RIGHT
        } else {
            CHEVRON_DOWN
        };
        let hover = palette.hover;
        let total = |count: usize, status: Status| {
            (count > 0).then(|| {
                div()
                    .text_color(palette.status(status))
                    .child(status_symbols(format!("{} {count}", status.glyph())))
            })
        };
        let selector = self.id.to_string();
        div()
            .debug_selector(move || selector)
            .id(self.id)
            .w_full()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .h(self.height)
            .pl(self.inset)
            .pr(self.trailing)
            .border_b(RULE)
            .border_color(palette.line)
            .font_family(FONT_MONO)
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .whitespace_nowrap()
            .cursor_pointer()
            .bg(if self.focused {
                palette.sel
            } else {
                palette.panel
            })
            .when(!self.focused, |row| row.hover(move |style| style.bg(hover)))
            .child(
                svg()
                    .path(chevron)
                    .size(ICON)
                    .flex_none()
                    .text_color(palette.mute),
            )
            .child(div().text_size(TEXT_AXIS).child(self.facet.to_owned()))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(TEXT_MONO)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.fg)
                    .child(self.group.value.clone()),
            )
            .child(self.group.rows.to_string())
            .children(total(self.group.errors, Status::Err))
            .children(total(self.group.warnings, Status::Warn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_read_as_capitalised_facets() {
        assert_eq!(option_label("namespace"), SharedString::from("Namespace"));
        assert_eq!(option_label("status"), SharedString::from("Status"));
        assert_eq!(option_label(""), SharedString::from(""));
    }
}
