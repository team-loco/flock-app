use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div,
};

use flock_core::units::Amount;

use super::{AmountText, ClickableSection, SectionLabel};
use crate::theme::{
    ActiveTheme, FONT_MONO, RULE, SPACE_3, SPACE_4, SPACE_5, SPACE_8, TEXT_BODY, TEXT_SMALL,
};

const SEPARATOR: &str = " · ";

pub type Activate = Rc<dyn Fn(&mut Window, &mut App)>;

pub fn problem_link(
    id: impl Into<SharedString>,
    content: Div,
    activate: Activate,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.to_string();
    content
        .debug_selector(move || selector)
        .id(ElementId::Name(id))
        .cursor_pointer()
        .hover(Styled::underline)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            activate(window, cx);
        })
}

pub struct TileNote {
    text: SharedString,
    color: Hsla,
    drill: Option<(SharedString, Activate)>,
}

impl TileNote {
    pub fn new(text: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            text: text.into(),
            color,
            drill: None,
        }
    }

    pub fn drill(mut self, id: impl Into<SharedString>, activate: Activate) -> Self {
        self.drill = Some((id.into(), activate));
        self
    }
}

#[derive(IntoElement)]
pub struct HealthTile {
    id: SharedString,
    label: SharedString,
    figure: Amount,
    detail: Option<SharedString>,
    bar: AnyElement,
    notes: Vec<TileNote>,
    on_activate: Option<Activate>,
}

impl HealthTile {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        figure: impl Into<SharedString>,
        bar: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            figure: Amount {
                figures: figure.into().to_string(),
                unit: None,
            },
            detail: None,
            bar: bar.into_any_element(),
            notes: Vec::new(),
            on_activate: None,
        }
    }

    pub fn amount(mut self, amount: Amount) -> Self {
        self.figure = amount;
        self
    }

    pub fn detail(mut self, detail: Option<impl Into<SharedString>>) -> Self {
        self.detail = detail.map(Into::into);
        self
    }

    pub fn notes(mut self, notes: Vec<TileNote>) -> Self {
        self.notes = notes;
        self
    }

    pub fn on_activate(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }
}

fn mono(text: SharedString, color: Hsla) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_color(color)
        .whitespace_nowrap()
        .child(text)
}

fn notes(parts: Vec<TileNote>, mute: Hsla) -> Div {
    let last = parts.len().saturating_sub(1);
    let spans = parts.into_iter().enumerate().flat_map(|(ix, note)| {
        let span = div().text_color(note.color).child(note.text);
        let span = if ix == last {
            span.min_w_0().truncate()
        } else {
            span.flex_none()
        };
        let span = match note.drill {
            Some((id, activate)) => problem_link(id, span, activate).into_any_element(),
            None => span.into_any_element(),
        };
        let separator = (ix < last).then(|| {
            div()
                .flex_none()
                .text_color(note.color)
                .child(SEPARATOR)
                .into_any_element()
        });
        std::iter::once(span).chain(separator)
    });
    div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .text_color(mute)
        .whitespace_nowrap()
        .children(spans)
}

impl RenderOnce for HealthTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let detail = self
            .detail
            .map(|detail| mono(detail, palette.mute).text_size(TEXT_SMALL).truncate());
        let title = div()
            .flex()
            .items_baseline()
            .gap(SPACE_4)
            .min_w_0()
            .child(SectionLabel::new(self.label))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_BODY)
                    .whitespace_nowrap()
                    .child(AmountText::new(self.figure).color(palette.fg)),
            )
            .children(detail);
        let notes = notes(self.notes, palette.mute);
        let Some(activate) = self.on_activate else {
            let id = self.id.to_string();
            return div()
                .debug_selector(|| id)
                .flex()
                .flex_col()
                .gap(SPACE_3)
                .px(SPACE_8)
                .py(SPACE_5)
                .child(title)
                .child(self.bar)
                .child(notes)
                .into_any_element();
        };
        ClickableSection::new(ElementId::Name(self.id))
            .padding(SPACE_8, SPACE_5)
            .gap(SPACE_3)
            .title(title)
            .child(self.bar)
            .child(notes)
            .on_activate(move |window, cx| activate(window, cx))
            .into_any_element()
    }
}

#[derive(IntoElement)]
pub struct HealthTiles {
    tiles: Vec<HealthTile>,
}

impl HealthTiles {
    pub fn new(tiles: Vec<HealthTile>) -> Self {
        Self { tiles }
    }
}

impl RenderOnce for HealthTiles {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let last = self.tiles.len().saturating_sub(1);
        let row = self.tiles.into_iter().enumerate().map(|(ix, tile)| {
            let column = div().flex().flex_col().flex_1().min_w_0().child(tile);
            if ix < last {
                column.border_r(RULE).border_color(palette.line)
            } else {
                column
            }
        });
        div()
            .flex()
            .border_b(RULE)
            .border_color(palette.line)
            .children(row)
    }
}
