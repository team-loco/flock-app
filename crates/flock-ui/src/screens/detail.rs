use std::rc::Rc;

use flock_core::related::{JOB, ListLink, Related};
use flock_core::resources::{EVENT_LIST, POD_LIST, Resource, ResourceKind};
use flock_core::settings::NamespaceSelection;
use flock_core::{Command, LogTarget, Page, Preferences, ResourceRef, Snapshot, Status};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, div,
};

use super::related::{OnPick, Pick, page_sections};
use crate::kit::scroll_viewport;
use crate::kit::{Button, CodeView, EmptyState, PageHeader, SectionHeader, Skeleton};
use crate::screens::EDIT_YAML;
use crate::screens::edit::{EDIT_KEYS, EditYaml, YAML_CONTEXT, edit_command};
use crate::theme::{ActiveTheme, SPACE_8};

const YAML: &str = "YAML";
const ALL_LOGS: &str = "Logs for all pods";
const SEPARATOR: &str = " · ";
const GONE: &str = "is not on this cluster any more.";
const BODY_ID: &str = "detail-body";

pub struct DetailScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    target: ResourceRef,
    row: Option<Resource>,
    related: Option<Related>,
    preferences: Preferences,
    loaded: bool,
    focus: FocusHandle,
}

impl EventEmitter<Command> for DetailScreen {}

impl Focusable for DetailScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn kind_of(target: &ResourceRef) -> Option<ResourceKind> {
    ResourceKind::ALL
        .into_iter()
        .find(|kind| kind.name() == target.kind)
}

fn list_key(page: Page) -> Option<&'static str> {
    match page {
        Page::Pods => Some(POD_LIST),
        Page::Events => Some(EVENT_LIST),
        other => ResourceKind::for_page(other).map(ResourceKind::name),
    }
}

impl DetailScreen {
    pub fn new(target: ResourceRef, cx: &mut Context<Self>) -> Self {
        Self {
            navigation: None,
            target,
            row: None,
            related: None,
            preferences: Preferences::default(),
            loaded: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn target(&self) -> &ResourceRef {
        &self.target
    }

    pub fn set(&mut self, snapshot: &Snapshot, preferences: &Preferences, cx: &mut Context<Self>) {
        let detail = snapshot.detail(&self.target.cluster);
        let row = detail
            .zip(kind_of(&self.target))
            .and_then(|(detail, kind)| {
                detail
                    .resources(kind)
                    .iter()
                    .find(|row| {
                        row.name == self.target.name
                            && Some(row.namespace.as_str()) == self.target.namespace.as_deref()
                    })
                    .cloned()
            })
            .or_else(|| {
                detail
                    .and_then(|detail| detail.object(&self.target))
                    .cloned()
            });
        let related = detail
            .and_then(|detail| detail.related(&self.target).cloned())
            .or_else(|| self.related.clone());
        let loaded = detail.is_some();
        let unchanged = self.row == row
            && self.related == related
            && self.loaded == loaded
            && &self.preferences == preferences;
        if unchanged {
            return;
        }
        self.row = row;
        self.related = related;
        self.loaded = loaded;
        self.preferences = preferences.clone();
        cx.notify();
    }

    fn pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Object(target) => cx.emit(Command::OpenResource(target)),
            Pick::List(link) => self.open_list(&link, cx),
        }
    }

    fn open_list(&mut self, link: &ListLink, cx: &mut Context<Self>) {
        if let Some(list) = list_key(link.page) {
            let selection = NamespaceSelection::Named(link.namespace.clone());
            self.preferences.select_namespace(list, selection);
            let cluster = self.target.cluster.clone();
            cx.emit(Command::SetPreferences(cluster, self.preferences.clone()));
        }
        cx.emit(Command::SelectPage(link.page));
    }

    fn status(&self) -> Option<(Status, String)> {
        let row = self.row.as_ref()?;
        let label = kind_of(&self.target).map_or_else(
            || row.cells.first().cloned().unwrap_or_default(),
            |kind| kind.status_cell(row).unwrap_or_default(),
        );
        Some((row.status, label))
    }

    fn meta(&self) -> String {
        let mut parts = vec![self.target.kind.clone()];
        parts.extend(self.target.namespace.clone());
        parts.join(SEPARATOR)
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let edit = Button::new("detail-edit", EDIT_YAML)
            .small()
            .tooltip(EDIT_KEYS)
            .on_click(cx.listener(|this, _, _, cx| this.edit(cx)));
        let logs = (self.target.kind == JOB).then(|| {
            Button::new("detail-logs", ALL_LOGS)
                .small()
                .on_click(cx.listener(|this, _, _, cx| {
                    let target = LogTarget::Workload(this.target.clone());
                    cx.emit(Command::FollowLogs(target));
                }))
        });
        let header = PageHeader::new(self.target.name.clone())
            .leading(self.navigation.clone())
            .mono()
            .meta(self.meta());
        let header = match self.status() {
            Some((status, label)) => header.status(status, label),
            None => header,
        };
        let header = match logs {
            Some(logs) => header.action(logs),
            None => header,
        };
        header.action(edit)
    }

    fn edit(&mut self, cx: &mut Context<Self>) {
        cx.emit(edit_command(self.target.clone()));
    }

    fn yaml(&self) -> Option<AnyElement> {
        let row = self.row.as_ref()?;
        let body: AnyElement = if row.yaml.is_empty() {
            Skeleton::rows().into_any_element()
        } else {
            CodeView::new("detail-yaml", row.yaml.clone())
                .yaml()
                .into_any_element()
        };
        let section = div()
            .flex()
            .flex_col()
            .child(SectionHeader::new(YAML))
            .child(div().px(SPACE_8).pb(SPACE_8).child(body));
        Some(section.into_any_element())
    }
}

impl Render for DetailScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let root = div()
            .id("detail")
            .key_context(YAML_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &EditYaml, _, cx| this.edit(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.editor)
            .child(self.header(cx));
        let missing = self.loaded && self.row.is_none();
        if missing {
            let sentence = format!("{} {} {GONE}", self.target.kind, self.target);
            return root.child(EmptyState::new(sentence).selector("detail-gone"));
        }
        let screen = cx.weak_entity();
        let pick: OnPick = Rc::new(move |pick, _, cx| {
            let _ = screen.update(cx, |screen, cx| screen.pick(pick, cx));
        });
        let related: Vec<AnyElement> = match &self.related {
            Some(related) => page_sections(related, &pick, cx),
            None => vec![Skeleton::rows().into_any_element()],
        };
        let content = div().children(related).children(self.yaml());
        root.child(scroll_viewport(BODY_ID, content).flex_1().min_h_0())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_links_scope_the_list_they_open() {
        assert_eq!(list_key(Page::Pods), Some(POD_LIST));
        assert_eq!(list_key(Page::Events), Some(EVENT_LIST));
        assert_eq!(list_key(Page::Pvcs), Some("PersistentVolumeClaim"));
        assert_eq!(list_key(Page::Nodes), None);
    }
}
