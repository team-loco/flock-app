use std::ops::Range;

use flock_core::Status;
use gpui::{
    AnyElement, App, ClipboardItem, ElementId, Entity, FocusHandle, HighlightStyle,
    InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, StyledText, TextLayout, TextRun, Window, div, font,
};

use super::button::Button;
use super::overlay::{Toast, toast};
use super::selectable::Selectable;
use crate::theme::{
    ActiveTheme, CODE_HEIGHT, CODE_LINE, CONTROL_SMALL, DIALOG_TOP, FONT_MONO, GUTTER_NUMBER,
    PAGE_HEADER, Palette, RADIUS, RULE, SPACE_2, SPACE_3, SPACE_4, SPACE_8, TEXT_MONO,
};

const COPY: &str = "Copy";
const PANEL_EDGES: f32 = 2.;
const COPIED: &str = "Copied to the clipboard";
const COMMENT: char = '#';
const KEY_END: &str = ": ";
const KEY_END_LINE: char = ':';
const LIST_ITEM: &str = "- ";
const SELECTABLE_ID: &str = "code-text";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Key,
    Punctuation,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Plain,
    Yaml,
}

fn comment_start(line: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (at, c) in line.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == COMMENT && previous.is_whitespace() => return Some(at),
            None => {}
        }
        previous = c;
    }
    None
}

pub fn yaml_tokens(line: &str) -> Vec<(Range<usize>, Token)> {
    let mut tokens = Vec::new();
    let comment = comment_start(line);
    let code_end = comment.unwrap_or(line.len());
    let code = line.get(..code_end).unwrap_or(line);
    let indent = code.len() - code.trim_start().len();
    let mut start = indent;
    if code
        .get(start..)
        .is_some_and(|rest| rest.starts_with(LIST_ITEM))
    {
        tokens.push((start..start + 1, Token::Punctuation));
        start += LIST_ITEM.len();
    }
    let body = code.get(start..).unwrap_or_default();
    let colon = body.find(KEY_END).or_else(|| {
        body.trim_end()
            .ends_with(KEY_END_LINE)
            .then(|| body.trim_end().len() - 1)
    });
    if let Some(colon) = colon {
        let key = body.get(..colon).unwrap_or_default();
        let quoted = key.starts_with('"') || key.starts_with('\'');
        if !key.is_empty() && (quoted || !key.contains(' ')) {
            tokens.push((start..start + colon, Token::Key));
            tokens.push((start + colon..start + colon + 1, Token::Punctuation));
        }
    }
    if let Some(at) = comment {
        tokens.push((at..line.len(), Token::Comment));
    }
    tokens
}

pub(super) fn token_style(token: Token, palette: &Palette) -> HighlightStyle {
    let color = match token {
        Token::Key => palette.acc,
        Token::Punctuation => palette.mute,
        Token::Comment => palette.mute,
    };
    HighlightStyle {
        color: Some(color),
        ..HighlightStyle::default()
    }
}

#[derive(IntoElement)]
pub struct CodeView {
    id: ElementId,
    text: SharedString,
    language: Language,
    copy: bool,
}

impl CodeView {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            language: Language::Plain,
            copy: true,
        }
    }

    pub fn yaml(mut self) -> Self {
        self.language = Language::Yaml;
        self
    }

    pub fn without_copy(mut self) -> Self {
        self.copy = false;
        self
    }
}

struct CodeState {
    wrap: bool,
    expanded: bool,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
}

fn styled_line(line: &str, language: Language, palette: &Palette) -> StyledText {
    let owned = SharedString::from(line.to_owned());
    match language {
        Language::Yaml => {
            let highlights = yaml_tokens(line)
                .into_iter()
                .map(|(range, token)| (range, token_style(token, palette)));
            StyledText::new(owned).with_highlights(highlights)
        }
        Language::Plain => StyledText::new(owned),
    }
}

fn code_width(text: &str, palette: &Palette, window: &Window) -> Pixels {
    let code_font = font(FONT_MONO);
    text.lines()
        .map(|line| {
            let runs = [TextRun {
                len: line.len(),
                font: code_font.clone(),
                color: palette.fg,
                background_color: None,
                underline: None,
                strikethrough: None,
            }];
            let shaped =
                window
                    .text_system()
                    .shape_line(line.to_owned().into(), TEXT_MONO, &runs, None);
            shaped.width
        })
        .max()
        .unwrap_or(Pixels::ZERO)
}

fn selectable(content: impl IntoElement, layouts: Vec<(SharedString, TextLayout)>) -> AnyElement {
    layouts
        .into_iter()
        .fold(
            Selectable::new(SELECTABLE_ID, content).whole_rows(),
            |selectable, (text, layout)| selectable.run(text, layout),
        )
        .into_any_element()
}

fn code_body(
    text: &str,
    language: Language,
    wrap: bool,
    palette: &Palette,
    height: Pixels,
    window: &Window,
) -> impl IntoElement + use<> {
    let mut layouts = Vec::new();
    let body = if wrap {
        let rows = text.lines().enumerate().map(|(ix, line)| {
            let number = div()
                .w(GUTTER_NUMBER)
                .flex_none()
                .justify_end()
                .flex()
                .pr(SPACE_4)
                .text_color(palette.mute)
                .child((ix + 1).to_string());
            let styled = styled_line(line, language, palette);
            layouts.push((SharedString::from(line.to_owned()), styled.layout().clone()));
            let text = div().flex_1().min_w_0().child(styled);
            div()
                .flex()
                .min_h(CODE_LINE)
                .line_height(CODE_LINE)
                .child(number)
                .child(text)
        });
        let column = div()
            .id("code-wrapped")
            .debug_selector(|| "code-wrapped".to_owned())
            .flex()
            .flex_col()
            .children(rows.collect::<Vec<_>>());
        selectable(column, layouts)
    } else {
        let width = code_width(text, palette, window);
        let numbers = text.lines().enumerate().map(|(ix, _)| {
            div()
                .h(CODE_LINE)
                .flex()
                .justify_end()
                .pr(SPACE_4)
                .child((ix + 1).to_string())
        });
        let lines = text.lines().map(|line| {
            let styled = styled_line(line, language, palette);
            layouts.push((SharedString::from(line.to_owned()), styled.layout().clone()));
            div()
                .h(CODE_LINE)
                .line_height(CODE_LINE)
                .whitespace_nowrap()
                .child(styled)
        });
        div()
            .flex()
            .child(
                div()
                    .id("code-gutter")
                    .debug_selector(|| "code-gutter".to_owned())
                    .w(GUTTER_NUMBER)
                    .flex_none()
                    .text_color(palette.mute)
                    .children(numbers),
            )
            .child(
                div()
                    .id("code-horizontal")
                    .debug_selector(|| "code-horizontal".to_owned())
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .on_scroll_wheel(|event, _, cx| {
                        let delta = event.delta.pixel_delta(CODE_LINE);
                        if delta.x.abs() > delta.y.abs() {
                            cx.stop_propagation();
                        }
                    })
                    .child(selectable(
                        div()
                            .id("code-lines")
                            .debug_selector(|| "code-lines".to_owned())
                            .w(width)
                            .flex()
                            .flex_col()
                            .children(lines.collect::<Vec<_>>()),
                        layouts,
                    )),
            )
            .into_any_element()
    };
    div()
        .id("code-vertical")
        .debug_selector(|| "code-vertical".to_owned())
        .min_h_0()
        .h(height)
        .overflow_y_scroll()
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .line_height(CODE_LINE)
        .text_color(palette.fg)
        .py(SPACE_3)
        .child(body)
}

fn tools(
    state: &Entity<CodeState>,
    text: SharedString,
    copy: bool,
    cx: &App,
) -> impl IntoElement + use<> {
    let wrapping = state.clone();
    let expanding = state.clone();
    div()
        .flex()
        .flex_none()
        .gap(SPACE_3)
        .p(SPACE_2)
        .child(
            Button::new("code-wrap", "Soft wrap")
                .small()
                .selected(state.read(cx).wrap)
                .on_click(move |_, window, cx| {
                    wrapping.update(cx, |state, _| state.wrap = !state.wrap);
                    window.refresh();
                }),
        )
        .child(
            Button::new("code-expand", "Expand")
                .small()
                .disabled(state.read(cx).expanded)
                .on_click(move |_, window, cx| {
                    expanding.update(cx, |state, cx| {
                        state.expanded = true;
                        state.return_focus = window.focused(cx);
                        state.focus.focus(window, cx);
                    });
                    window.refresh();
                }),
        )
        .children(copy.then(|| {
            Button::new("copy", COPY).small().on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                toast(Toast::new(Status::Ok, COPIED), cx);
            })
        }))
}

impl RenderOnce for CodeView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = *cx.palette();
        let state =
            window.use_keyed_state((self.id.clone(), "code-state"), cx, |_, cx| CodeState {
                wrap: false,
                expanded: false,
                focus: cx.focus_handle(),
                return_focus: None,
            });
        let wrap = state.read(cx).wrap;
        let expanded = state.read(cx).expanded;
        let overlay = expanded.then(|| {
            let closing = state.clone();
            let focus = state.read(cx).focus.clone();
            super::modal::Modal::new(
                "expanded-yaml",
                "YAML",
                window.viewport_size().width - SPACE_8,
            )
            .focus(&focus)
            .on_close(move |window, cx| {
                closing.update(cx, |state, cx| {
                    state.expanded = false;
                    if let Some(focus) = state.return_focus.take() {
                        focus.focus(window, cx);
                    }
                });
                window.refresh();
            })
            .child(tools(&state, self.text.clone(), self.copy, cx))
            .child(code_body(
                &self.text,
                self.language,
                wrap,
                &palette,
                window.viewport_size().height
                    - DIALOG_TOP * PANEL_EDGES
                    - PAGE_HEADER
                    - CONTROL_SMALL,
                window,
            ))
            .into_any_element()
        });
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(RADIUS)
            .border(RULE)
            .border_color(palette.line)
            .bg(palette.editor)
            .child(tools(&state, self.text.clone(), self.copy, cx))
            .child(code_body(
                &self.text,
                self.language,
                wrap,
                &palette,
                CODE_HEIGHT,
                window,
            ))
            .children(overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(&str, Token)> {
        yaml_tokens(line)
            .into_iter()
            .map(|(range, token)| (&line[range], token))
            .collect()
    }

    #[test]
    fn keys_values_and_comments() {
        assert_eq!(
            kinds("  image: ghcr.io/acme/api:2.14 # pinned"),
            [
                ("image", Token::Key),
                (":", Token::Punctuation),
                ("# pinned", Token::Comment),
            ]
        );
        assert_eq!(
            kinds("- name: STRIPE_WEBHOOK_SECRET"),
            [
                ("-", Token::Punctuation),
                ("name", Token::Key),
                (":", Token::Punctuation),
            ]
        );
        assert_eq!(
            kinds("spec:"),
            [("spec", Token::Key), (":", Token::Punctuation)]
        );
    }

    #[test]
    fn hashes_inside_values_are_not_comments() {
        assert_eq!(
            kinds("url: \"http://x/#frag\""),
            [("url", Token::Key), (":", Token::Punctuation)]
        );
        assert_eq!(
            kinds("tag: a#b"),
            [("tag", Token::Key), (":", Token::Punctuation)]
        );
    }

    #[test]
    fn plain_scalars_are_left_alone() {
        assert!(
            kinds("    - app.yaml")
                .iter()
                .all(|(_, t)| *t == Token::Punctuation)
        );
        assert_eq!(kinds("just text: with spaces in key"), []);
        assert_eq!(kinds("# whole line"), [("# whole line", Token::Comment)]);
    }
}
