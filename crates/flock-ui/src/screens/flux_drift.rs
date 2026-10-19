use std::rc::Rc;

use flock_core::flux::{Drift, DriftField, DriftState, DriftedObject};
use flock_core::{ClusterId, ResourceRef, Status, time};
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder,
};

use crate::kit::{SectionHeader, StatusGlyph};
use crate::theme::{
    FONT_MONO, Palette, RULE, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_8,
    TEXT_MONO, TEXT_SMALL,
};

const TITLE: &str = "Drift";
const SELECTOR: &str = "flux-drift";
const OBJECT_SELECTOR: &str = "flux-drift-object";
const CHECKING: &str = "Checking the live objects against Flux's inventory…";
const UNREADABLE: &str = "Could not read:";
const METHOD: &str = "Compares Flux's inventory with the live objects. A changed field is one another field manager wrote after kustomize-controller last applied the object. Flux takes back fields written with kubectl at its next reconcile; changes made by other managers stay.";
const SEPARATOR: &str = ", ";

pub type OpenResource = Rc<dyn Fn(ResourceRef, &mut Window, &mut App)>;

fn summary(drift: &Drift) -> String {
    let objects = if drift.checked == 1 {
        "object"
    } else {
        "objects"
    };
    if drift.objects.is_empty() {
        return format!("none · {} {objects} checked", drift.checked);
    }
    format!(
        "{} · {} {objects} checked",
        drift.objects.len(),
        drift.checked
    )
}

fn clean(drift: &Drift) -> String {
    let (objects, verb) = if drift.checked == 1 {
        ("object", "matches")
    } else {
        ("objects", "match")
    };
    format!(
        "No drift: {} {objects} {verb} what kustomize-controller last applied.",
        drift.checked
    )
}

fn field_row(field: &DriftField, palette: &Palette) -> AnyElement {
    let when = field.age.map_or_else(
        || format!(" · {}", field.manager),
        |age| format!(" · {} {} ago", field.manager, time::age(age)),
    );
    div()
        .flex()
        .flex_wrap()
        .gap(SPACE_2)
        .pl(SPACE_8)
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .child(div().text_color(palette.fg2).child(field.path.clone()))
        .child(
            div()
                .text_color(palette.fg)
                .child(format!("= {}", field.value)),
        )
        .child(div().text_color(palette.mute).child(when))
        .into_any_element()
}

fn object_row(
    ix: usize,
    object: &DriftedObject,
    cluster: &ClusterId,
    open: &OpenResource,
    palette: &Palette,
) -> AnyElement {
    let linked = object.state != DriftState::Missing;
    let target = object.resource(cluster);
    let open = open.clone();
    let acc = palette.acc;
    let path = div()
        .id((OBJECT_SELECTOR, ix))
        .debug_selector(move || format!("{OBJECT_SELECTOR}-{ix}"))
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .text_color(if linked { palette.acc } else { palette.fg })
        .when(linked, |this| {
            this.cursor_pointer()
                .hover(move |style| style.text_color(acc).underline())
                .on_click(move |_, window, cx| open(target.clone(), window, cx))
        })
        .child(object.path());
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .py(SPACE_3)
        .border_b(RULE)
        .border_color(palette.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .child(StatusGlyph::new(object.state.status()))
                .child(path)
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(object.state.label()),
                ),
        )
        .children(object.fields.iter().map(|field| field_row(field, palette)))
        .into_any_element()
}

pub fn section(
    drift: Option<&Drift>,
    cluster: &ClusterId,
    open: &OpenResource,
    palette: &Palette,
) -> AnyElement {
    let header = match drift {
        Some(drift) => SectionHeader::new(TITLE).count(summary(drift)),
        None => SectionHeader::new(TITLE),
    };
    let body: Vec<AnyElement> = match drift {
        None => vec![
            div()
                .text_color(palette.mute)
                .child(CHECKING)
                .into_any_element(),
        ],
        Some(drift) if drift.objects.is_empty() => vec![
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .text_color(palette.fg2)
                .child(StatusGlyph::new(Status::Ok))
                .child(clean(drift))
                .into_any_element(),
        ],
        Some(drift) => drift
            .objects
            .iter()
            .enumerate()
            .map(|(ix, object)| object_row(ix, object, cluster, open, palette))
            .collect(),
    };
    let unreadable = drift
        .filter(|drift| !drift.unreadable.is_empty())
        .map(|drift| {
            div()
                .pt(SPACE_3)
                .text_color(palette.mute)
                .child(format!("{UNREADABLE} {}", drift.unreadable.join(SEPARATOR)))
        });
    div()
        .debug_selector(|| SELECTOR.to_owned())
        .child(header)
        .child(
            div()
                .px(SPACE_8)
                .pb(SPACE_5)
                .flex()
                .flex_col()
                .children(body)
                .children(unreadable)
                .child(
                    div()
                        .pt(SPACE_4)
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(METHOD),
                ),
        )
        .pb(SPACE_6)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_counts_drifted_and_checked_objects() {
        let mut drift = Drift {
            checked: 3,
            ..Drift::default()
        };
        assert_eq!(summary(&drift), "none · 3 objects checked");
        drift.objects.push(DriftedObject {
            state: DriftState::Missing,
            kind: "Service".into(),
            namespace: Some("web".into()),
            name: "podinfo".into(),
            fields: Vec::new(),
        });
        assert_eq!(summary(&drift), "1 · 3 objects checked");
        drift.checked = 1;
        assert_eq!(
            clean(&drift),
            "No drift: 1 object matches what kustomize-controller last applied."
        );
    }
}
