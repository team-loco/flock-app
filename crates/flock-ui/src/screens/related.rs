use std::rc::Rc;

use flock_core::ResourceRef;
use flock_core::related::{Group, Item, ListLink, Related};
use flock_core::time::age;
use gpui::{
    AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div,
};

use crate::kit::{Link, Rule, SectionHeader, Skeleton, StatusGlyph};
use crate::theme::{
    ActiveTheme, FONT_MONO, GLYPH_SLOT, Palette, ROW_DOCUMENT, SPACE_2, SPACE_4, SPACE_5, SPACE_8,
    TEXT_MONO, TEXT_SMALL,
};

pub const RELATED: &str = "Related";
const NOTHING: &str = "Nothing else in the cluster refers to it.";
const NONE: &str = "None";
const OPEN_LIST: &str = "Open list →";
const SELECTOR: &str = "related";

pub enum Pick {
    Object(ResourceRef),
    List(ListLink),
}

pub type OnPick = Rc<dyn Fn(Pick, &mut Window, &mut App)>;

pub fn selector(group: &str, target: &ResourceRef) -> String {
    format!("{SELECTOR}-{group}-{}", target.name)
}

fn list_selector(link: &ListLink) -> String {
    format!("{SELECTOR}-list-{}", link.page.label())
}

fn more(hidden: usize) -> String {
    format!("+{hidden} more")
}

fn row(group: &str, item: &Item, pick: &OnPick, palette: &Palette) -> AnyElement {
    let glyph: AnyElement = match item.status {
        Some(status) => StatusGlyph::new(status).into_any_element(),
        None => div().w(GLYPH_SLOT).flex_none().into_any_element(),
    };
    let name: AnyElement = if item.linked {
        let pick = pick.clone();
        let target = item.target.clone();
        let id = SharedString::from(selector(group, &item.target));
        Link::new(id, item.label.clone())
            .mono()
            .on_click(move |_, window, cx| pick(Pick::Object(target.clone()), window, cx))
            .into_any_element()
    } else {
        div()
            .font_family(FONT_MONO)
            .text_color(palette.fg2)
            .child(item.label.clone())
            .into_any_element()
    };
    let note = div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(palette.mute)
        .child(item.note.clone());
    let age = item.age.map(|elapsed| {
        div()
            .flex_none()
            .font_family(FONT_MONO)
            .text_color(palette.mute)
            .child(age(elapsed))
    });
    div()
        .flex()
        .items_center()
        .gap(SPACE_4)
        .h(ROW_DOCUMENT)
        .min_w_0()
        .text_size(TEXT_MONO)
        .child(glyph)
        .child(div().min_w_0().overflow_hidden().child(name))
        .child(note)
        .children(age)
        .into_any_element()
}

fn rows(group: &Group, pick: &OnPick, palette: &Palette) -> Vec<AnyElement> {
    let mut rows: Vec<AnyElement> = group
        .items
        .iter()
        .map(|item| row(&group.title, item, pick, palette))
        .collect();
    let hidden = group.hidden();
    if hidden > 0 {
        let line = div()
            .h(ROW_DOCUMENT)
            .flex()
            .items_center()
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .child(more(hidden));
        rows.push(line.into_any_element());
    }
    rows
}

pub fn inspector_section(related: Option<&Related>, pick: &OnPick, cx: &App) -> AnyElement {
    let palette = cx.palette();
    let Some(related) = related else {
        return Skeleton::rows().into_any_element();
    };
    if related.groups.is_empty() {
        return div()
            .text_color(palette.mute)
            .child(NOTHING)
            .into_any_element();
    }
    let groups = related.groups.iter().map(|group| {
        let title = div()
            .flex()
            .items_center()
            .gap(SPACE_4)
            .text_size(TEXT_SMALL)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.fg2)
                    .child(group.title.clone()),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_color(palette.mute)
                    .child(group.total.to_string()),
            );
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .child(title)
            .children(rows(group, pick, palette))
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_5)
        .children(groups)
        .into_any_element()
}

pub fn page_sections(related: &Related, pick: &OnPick, cx: &App) -> Vec<AnyElement> {
    let palette = cx.palette();
    if related.groups.is_empty() {
        let nothing = div()
            .flex()
            .flex_col()
            .child(SectionHeader::new(RELATED))
            .child(Rule::horizontal())
            .child(
                div()
                    .px(SPACE_8)
                    .py(SPACE_5)
                    .text_color(palette.mute)
                    .child(NOTHING),
            );
        return vec![nothing.into_any_element()];
    }
    related
        .groups
        .iter()
        .map(|group| {
            let link = group.list.clone().map(|list| {
                let pick = pick.clone();
                let id = SharedString::from(list_selector(&list));
                Link::new(id, OPEN_LIST)
                    .on_click(move |_, window, cx| pick(Pick::List(list.clone()), window, cx))
            });
            let header = SectionHeader::new(group.title.clone()).count(group.total.to_string());
            let header = match link {
                Some(link) => header.end(link),
                None => header,
            };
            let body: Vec<AnyElement> = if group.total == 0 {
                let none = div()
                    .h(ROW_DOCUMENT)
                    .flex()
                    .items_center()
                    .text_color(palette.mute)
                    .child(NONE);
                vec![none.into_any_element()]
            } else {
                rows(group, pick, palette)
            };
            div()
                .flex()
                .flex_col()
                .child(header)
                .child(Rule::horizontal())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .px(SPACE_8)
                        .py(SPACE_2)
                        .children(body),
                )
                .into_any_element()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_name_the_object_and_the_list() {
        let target = ResourceRef {
            cluster: flock_core::ClusterId::new("a"),
            kind: "Deployment".into(),
            namespace: Some("payments".into()),
            name: "checkout-api".into(),
        };
        assert_eq!(
            selector("Deployments", &target),
            "related-Deployments-checkout-api"
        );
        let link = ListLink {
            page: flock_core::Page::Pvcs,
            namespace: "payments".into(),
        };
        assert_eq!(list_selector(&link), "related-list-PVCs");
        assert_eq!(more(3), "+3 more");
    }
}
