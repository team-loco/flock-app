use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, HighlightStyle, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, Window, div,
};
use gpui_base::input::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::{
    Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Position,
    Rope,
};

use super::code::{Token, token_style, yaml_tokens};
use crate::theme::{FONT_MONO, Palette, TEXT_MONO};

pub use gpui_component::input::EditorState as CodeEditorState;

const YAML: &str = "yaml";

pub fn yaml_editor(window: &mut Window, cx: &mut Context<EditorState>) -> EditorState {
    EditorState::new(window, cx)
        .language(YAML)
        .line_number(true)
        .folding(false)
}

pub fn highlight_yaml(state: &mut EditorState, palette: Palette, cx: &mut Context<EditorState>) {
    let factory = Rc::new(move |language: &str| {
        (language == YAML).then(|| {
            Box::new(YamlHighlighter {
                palette,
                spans: Vec::new(),
            }) as Box<dyn InputHighlighter>
        })
    });
    state.set_highlighter_factory(factory, cx);
}

pub fn mark_error(
    state: &mut EditorState,
    line: Option<usize>,
    message: Option<&str>,
    cx: &mut Context<EditorState>,
) {
    let text = state.text().clone();
    let Some(diagnostics) = state.diagnostics_mut() else {
        return;
    };
    diagnostics.reset(&text);
    if let Some(message) = message {
        let row = line.unwrap_or_default();
        let length = text.to_string().lines().nth(row).map_or(0, str::len);
        let start = Position::new(position(row), 0);
        let end = Position::new(position(row), position(length));
        let diagnostic = Diagnostic::new(start..end, SharedString::from(message.to_owned()))
            .with_severity(DiagnosticSeverity::Error);
        diagnostics.push(diagnostic);
    }
    cx.notify();
}

fn position(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

struct YamlHighlighter {
    palette: Palette,
    spans: Vec<(Range<usize>, Token)>,
}

impl InputHighlighter for YamlHighlighter {
    fn language(&self) -> SharedString {
        SharedString::new_static(YAML)
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.spans = spans(&text.to_string());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut runs = Vec::new();
        let mut at = range.start;
        let inside = self
            .spans
            .iter()
            .filter(|(span, _)| span.end > range.start && span.start < range.end);
        for (span, token) in inside {
            let start = span.start.max(at);
            let end = span.end.min(range.end);
            if start >= end {
                continue;
            }
            if start > at {
                runs.push((at..start, HighlightStyle::default()));
            }
            runs.push((start..end, token_style(*token, &self.palette)));
            at = end;
        }
        if at < range.end {
            runs.push((at..range.end, HighlightStyle::default()));
        }
        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

fn spans(text: &str) -> Vec<(Range<usize>, Token)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches('\n');
        for (range, token) in yaml_tokens(content) {
            spans.push((offset + range.start..offset + range.end, token));
        }
        offset += line.len();
    }
    spans
}

#[derive(IntoElement)]
pub struct CodeEditor {
    state: Entity<EditorState>,
}

impl CodeEditor {
    pub fn new(state: &Entity<EditorState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl RenderOnce for CodeEditor {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .size_full()
            .font_family(FONT_MONO)
            .text_size(TEXT_MONO)
            .child(
                Editor::new(&self.state)
                    .appearance(false)
                    .bordered(false)
                    .h_full()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_MONO),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_follow_each_line_offset() {
        let text = "kind: ConfigMap\ndata:\n  level: info # note\n";
        let keys: Vec<&str> = spans(text)
            .into_iter()
            .filter(|(_, token)| *token == Token::Key)
            .filter_map(|(range, _)| text.get(range))
            .collect();
        assert_eq!(keys, ["kind", "data", "level"]);
        let comments: Vec<&str> = spans(text)
            .into_iter()
            .filter(|(_, token)| *token == Token::Comment)
            .filter_map(|(range, _)| text.get(range))
            .collect();
        assert_eq!(comments, ["# note"]);
    }
}
