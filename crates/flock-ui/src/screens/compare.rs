use crate::kit::SelectableText;
use flock_core::changes::LineChange;
use flock_core::compare::{Cell, Comparison, Field, SpecDiff};
use flock_core::overview::plural;
use flock_core::{ClusterId, Command, Page, ResourceRef, Snapshot, Status};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::kit::{
    Button, ConfirmDialog, DiffFile, DiffLine, DiffView, LineKind, PageHeader, SectionHeader,
    StatusGlyph, Toggle, confirm,
};
use crate::theme::{
    ActiveTheme, FONT_MONO, PAGE_HEADER, Palette, RULE, SPACE_1, SPACE_2, SPACE_4, SPACE_5,
    SPACE_7, SPACE_8, TEXT_AXIS, TEXT_BODY, TEXT_CONTROL, TEXT_MONO, TEXT_SMALL, TEXT_TITLE,
};

const FIELD_COLUMN: Pixels = px(200.);
const CLUSTER_COLUMN_MIN: Pixels = px(180.);

const DIFFERENCES_ONLY: &str = "Differences only";
const FIELD: &str = "Field";
const DIFFERS: &str = "differs";
const LIVE_NOTE: &str = "live values, refreshed by watch";
const APPLY: &str = "Apply";
const SEPARATOR: &str = " · ";
const VERSUS: &str = " vs ";

pub struct CompareScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    resource: Option<ResourceRef>,
    comparison: Option<Comparison>,
    confirm_on_target: bool,
    differences_only: bool,
}

impl EventEmitter<Command> for CompareScreen {}

impl CompareScreen {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            navigation: None,
            resource: None,
            comparison: None,
            confirm_on_target: true,
            differences_only: true,
        }
    }

    pub fn set_snapshot(
        &mut self,
        snapshot: &Snapshot,
        resource: Option<&ResourceRef>,
        cx: &mut Context<Self>,
    ) {
        self.resource = resource.cloned();
        self.comparison = resource.and_then(|r| snapshot.comparison(r)).cloned();
        let target = self
            .comparison
            .as_ref()
            .and_then(|c| c.spec.as_ref())
            .and_then(|spec| snapshot.catalog.get(&spec.base));
        self.confirm_on_target = target.is_none_or(|s| s.preferences.confirm_destructive);
        cx.notify();
    }

    fn header(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let subject = self.resource.as_ref().map(|r| {
            let namespace = r.namespace.as_deref().unwrap_or_default();
            format!("{}{SEPARATOR}{namespace}/{}", r.kind, r.name)
        });
        let toggle = Toggle::new("compare-differences", self.differences_only)
            .label(DIFFERENCES_ONLY)
            .on_toggle(cx.listener(|this, on: &bool, _, cx| {
                this.differences_only = *on;
                cx.notify();
            }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_4)
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .text_size(TEXT_BODY)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .children(self.navigation.clone())
            .child(
                div()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Page::Compare.label()),
            )
            .children(subject.map(|s| {
                div()
                    .pl(SPACE_2)
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .text_color(palette.mute)
                    .child(s)
            }))
            .child(div().flex_1())
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(toggle),
            )
    }

    fn apply(&self, spec: &SpecDiff, comparison: &Comparison, cx: &mut Context<Self>) -> Button {
        let seen = comparison.seen_on(&spec.base);
        let target = seen.target.clone();
        let command = Command::CopySpec {
            from: spec.other.clone(),
            to: seen,
            path: spec.path.clone(),
        };
        let this = cx.entity().downgrade();
        let send = move |cx: &mut gpui::App| {
            let command = command.clone();
            let sent = this.update(cx, |_, cx| cx.emit(command));
            if sent.is_err() {
                tracing::debug!("compare screen closed before the copy was sent");
            }
        };
        let needs_confirm = self.confirm_on_target;
        let dialog = confirm_copy(spec, &target, send.clone());
        Button::new("compare-apply", spec.apply_label())
            .primary()
            .on_click(move |_, window, cx| {
                if needs_confirm {
                    confirm(dialog.clone(), window, cx);
                } else {
                    send(cx);
                }
            })
    }
}

impl Render for CompareScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let body = match self.comparison.clone() {
            Some(comparison) => self.document(&comparison, &palette, cx),
            None => empty(self.resource.as_ref(), &palette),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.header(&palette, cx))
            .child(
                div()
                    .id("compare-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}

impl CompareScreen {
    fn document(
        &self,
        comparison: &Comparison,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clusters = plural(comparison.clusters.len(), "cluster", "clusters");
        let meta = format!("{clusters}{SEPARATOR}{LIVE_NOTE}");
        let fields = comparison.visible(self.differences_only);
        let heads = cluster_row(comparison, self.resource.as_ref(), palette, cx);
        let spec = comparison
            .spec
            .as_ref()
            .map(|spec| self.spec_section(spec, comparison, palette, cx));
        div()
            .flex()
            .flex_col()
            .child(
                PageHeader::new(comparison.name.clone())
                    .mono()
                    .sentence(SelectableText::new(comparison.sentence()))
                    .meta(meta),
            )
            .child(heads)
            .children(fields.into_iter().map(|f| field_row(f, palette)))
            .child(
                div()
                    .px(SPACE_8)
                    .py(SPACE_4)
                    .border_b(RULE)
                    .border_color(palette.line)
                    .text_size(TEXT_CONTROL)
                    .text_color(palette.mute)
                    .child(comparison.hidden_note(self.differences_only)),
            )
            .children(spec)
            .into_any_element()
    }

    fn spec_section(
        &self,
        spec: &SpecDiff,
        comparison: &Comparison,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pair = format!("{}{VERSUS}{}", spec.base, spec.other);
        let header = SectionHeader::new(spec.path.clone()).count(pair);
        let header = if spec.has_changes() {
            header.end(self.apply(spec, comparison, cx))
        } else {
            header
        };
        let path = format!(
            "{}/{}{SEPARATOR}{}",
            comparison.kind, comparison.name, spec.path
        );
        let lines = spec.lines.iter().map(diff_line).collect();
        let file = DiffFile::new(path, lines);
        let note = (!spec.has_changes()).then(|| {
            div()
                .text_size(TEXT_CONTROL)
                .text_color(palette.mute)
                .child(format!(
                    "{} is identical on {} and {}.",
                    spec.path, spec.base, spec.other
                ))
        });
        div().flex().flex_col().child(header).child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_4)
                .px(SPACE_8)
                .pb(SPACE_7)
                .children(note)
                .child(
                    DiffView::new("compare-diff", vec![file])
                        .sides(spec.base.to_string(), spec.other.to_string()),
                ),
        )
    }
}

fn confirm_copy(
    spec: &SpecDiff,
    target: &ResourceRef,
    send: impl Fn(&mut gpui::App) + 'static,
) -> ConfirmDialog {
    let title = format!("Apply {} value to {}", spec.other, spec.base);
    let body = format!(
        "{}'s {} is patched to match {}. The {} rolls out a new revision now; Flux may revert the change on its next reconcile.",
        spec.base, spec.path, spec.other, target.kind
    );
    let changed: Vec<&str> = spec
        .lines
        .iter()
        .filter(|l| l.change == LineChange::Added)
        .map(|l| l.text.trim())
        .collect();
    let detail = format!("{}{SEPARATOR}{target}", spec.base);
    changed.into_iter().fold(
        ConfirmDialog::new(title, body, APPLY, move |_, cx| send(cx)).detail(detail),
        ConfirmDialog::detail,
    )
}

fn empty_sentence(resource: Option<&ResourceRef>) -> String {
    match resource {
        Some(r) => format!("No other cluster reports {} {r}.", r.kind),
        None => "Pick a workload to compare it across clusters.".into(),
    }
}

fn empty(resource: Option<&ResourceRef>, palette: &Palette) -> AnyElement {
    let sentence = empty_sentence(resource);
    div()
        .px(SPACE_8)
        .pt(SPACE_7)
        .text_color(palette.fg2)
        .child(sentence)
        .into_any_element()
}

fn row(palette: &Palette) -> gpui::Div {
    div().flex().border_b(RULE).border_color(palette.line)
}

fn label_cell(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .w(FIELD_COLUMN)
        .flex_none()
        .pl(SPACE_8)
        .pr(SPACE_5)
        .py(SPACE_5)
        .text_color(palette.fg2)
}

fn value_cell(palette: &Palette) -> gpui::Div {
    div()
        .flex_1()
        .min_w(CLUSTER_COLUMN_MIN)
        .px(SPACE_8)
        .py(SPACE_5)
        .border_l(RULE)
        .border_color(palette.line)
}

pub fn pick(comparison: &Comparison, base: &ResourceRef, cluster: &ClusterId) -> Option<Command> {
    let current = comparison.spec.as_ref().map(|spec| &spec.other);
    if cluster == &base.cluster || current == Some(cluster) {
        return None;
    }
    Some(Command::CompareWith {
        base: base.clone(),
        other: cluster.clone(),
    })
}

fn cluster_row(
    comparison: &Comparison,
    base: Option<&ResourceRef>,
    palette: &Palette,
    cx: &mut Context<CompareScreen>,
) -> AnyElement {
    let other = comparison.spec.as_ref().map(|spec| spec.other.clone());
    let heads: Vec<AnyElement> = comparison
        .clusters
        .iter()
        .map(|c| {
            let command = base.and_then(|base| pick(comparison, base, &c.cluster));
            let chosen = other.as_ref() == Some(&c.cluster);
            let hover = palette.hover;
            let head = value_cell(palette)
                .id(SharedString::from(format!("compare-head-{}", c.cluster)))
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .when(chosen, |head| head.bg(palette.panel))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_4)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(StatusGlyph::new(c.status))
                        .child(div().truncate().child(c.cluster.to_string())),
                )
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .text_color(palette.status(c.status))
                        .truncate()
                        .child(c.health.clone()),
                );
            match command {
                Some(command) => head
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(command.clone())))
                    .into_any_element(),
                None => head.into_any_element(),
            }
        })
        .collect();
    row(palette)
        .child(
            label_cell(palette)
                .items_end()
                .text_size(TEXT_AXIS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.mute)
                .child(FIELD.to_uppercase()),
        )
        .children(heads)
        .into_any_element()
}

fn field_row(field: &Field, palette: &Palette) -> impl IntoElement {
    let differs = field.differs().then(|| {
        div()
            .flex_none()
            .font_family(FONT_MONO)
            .text_size(TEXT_AXIS)
            .text_color(palette.warn)
            .child(DIFFERS)
    });
    row(palette)
        .child(
            label_cell(palette)
                .child(div().flex_1().min_w_0().child(field.label.clone()))
                .children(differs),
        )
        .children(field.cells.iter().map(|cell| value(cell, palette)))
}

fn value(cell: &Cell, palette: &Palette) -> impl IntoElement {
    let text_color = match cell.flag {
        Some(Status::Err) => palette.err,
        _ => palette.fg,
    };
    let tint = cell.flag.and_then(|status| palette.tint(status));
    value_cell(palette)
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .text_color(text_color)
        .when_some(tint, Styled::bg)
        .child(SharedString::from(cell.text.clone()))
}

pub(crate) fn diff_line(line: &flock_core::changes::PatchLine) -> DiffLine {
    let kind = match line.change {
        LineChange::Context => LineKind::Context,
        LineChange::Added => LineKind::Added,
        LineChange::Removed => LineKind::Removed,
        LineChange::Hunk => LineKind::Hunk,
    };
    DiffLine::new(kind, Some(line.number), line.text.clone())
}

#[cfg(test)]
mod tests {
    use flock_core::Uid;
    use flock_core::changes::PatchLine;
    use flock_core::compare::ComparedCluster;

    use super::*;

    #[test]
    fn spec_lines_keep_their_side() {
        let removed = PatchLine {
            change: LineChange::Removed,
            number: 39,
            text: "key: webhook_secret".into(),
            note: None,
        };
        let converted = diff_line(&removed);
        assert_eq!(converted.kind, LineKind::Removed);
        assert_eq!(converted.old, Some(39));
    }

    #[test]
    fn clicking_a_cluster_head_makes_it_the_other_side() {
        let base = ResourceRef {
            cluster: ClusterId::new("a"),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "cart".into(),
        };
        let head = |name: &str| ComparedCluster {
            cluster: ClusterId::new(name),
            uid: Uid::default(),
            status: Status::Ok,
            health: "Healthy".into(),
        };
        let comparison = Comparison {
            kind: base.kind.clone(),
            namespace: base.namespace.clone(),
            name: base.name.clone(),
            clusters: vec![head("a"), head("b"), head("c")],
            fields: Vec::new(),
            spec: Some(SpecDiff {
                path: "spec.template".into(),
                base: ClusterId::new("a"),
                other: ClusterId::new("b"),
                lines: Vec::new(),
            }),
        };
        assert_eq!(pick(&comparison, &base, &ClusterId::new("a")), None);
        assert_eq!(pick(&comparison, &base, &ClusterId::new("b")), None);
        assert_eq!(
            pick(&comparison, &base, &ClusterId::new("c")),
            Some(Command::CompareWith {
                base,
                other: ClusterId::new("c"),
            })
        );
    }

    #[test]
    fn empty_sentence_names_the_resource() {
        let resource = ResourceRef {
            cluster: ClusterId::new("staging"),
            kind: "Deployment".into(),
            namespace: Some("shop".into()),
            name: "cart".into(),
        };
        assert_eq!(
            empty_sentence(Some(&resource)),
            "No other cluster reports Deployment shop/cart."
        );
        assert_eq!(
            empty_sentence(None),
            "Pick a workload to compare it across clusters."
        );
    }
}
