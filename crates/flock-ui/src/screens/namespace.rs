use std::collections::BTreeSet;
use std::rc::Rc;

use flock_core::events::Event;
use flock_core::home::Problem;
use flock_core::namespace::{Listing, NamespacePage, Policy, Quota, Requested, Tile};
use flock_core::related::{Group, Item};
use flock_core::resource::short_kind;
use flock_core::resources::{EVENT_LIST, namespace_list};
use flock_core::settings::NamespaceSelection;
use flock_core::time::age;
use flock_core::{Command, Page, Preferences, ResourceRef, Snapshot, Status};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder, relative, svg,
};

use crate::assets::{CHEVRON_DOWN, CHEVRON_RIGHT};
use crate::kit::{
    ActionMenu, Activate, EmptyState, HealthTile, HealthTiles, InputEvent, InspectorPane, Link,
    PageHeader, SectionLabel, Skeleton, StackedBar, StatusGlyph, TextField, TextFieldState,
    TileNote, Tooltip,
};
use crate::screens::edit::{EditYaml, YAML_CONTEXT, edit_command};
use crate::screens::{ScreenEvent, edit_item};
use crate::theme::{
    ASIDE_WIDTH, ActiveTheme, FONT_MONO, ICON, NAMESPACE_AGE_WIDTH, NAMESPACE_CONTENT_AGE,
    NAMESPACE_CONTENT_NAME, NAMESPACE_EVENT_AGE, NAMESPACE_EVENT_LINES, NAMESPACE_FILTER_WIDTH,
    NAMESPACE_KIND_WIDTH, NAMESPACE_NAME_WIDTH, NAMESPACE_USAGE_BAR, NAMESPACE_USAGE_LABEL,
    Palette, RADIUS, ROW_TABLE, ROW_TABLE_MAX, RULE, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5,
    SPACE_6, SPACE_7, SPACE_8, TEXT_BODY, TEXT_LABEL, TEXT_MONO, TEXT_SMALL,
};

const KICKER: &str = "Namespace";
const SHORT_KICKER: &str = "ns";
const ACTIONS: &str = "namespace-actions";
const NEEDS_ATTENTION: &str = "Needs attention";
const NOTHING: &str = "Nothing needs attention in this namespace.";
const CONTENTS: &str = "Contents";
const FILTER: &str = "Filter by name";
const NO_MATCH: &str = "Nothing in this namespace matches the filter.";
const EMPTY: &str = "Empty: ";
const USAGE: &str = "Usage";
const CPU: &str = "CPU";
const MEMORY: &str = "Memory";
const QUOTA: &str = "Quota";
const LABELS: &str = "Labels";
const POLICIES: &str = "Network policies";
const EVENTS: &str = "Recent events";
const ALL: &str = "All";
const NONE: &str = "None";
const LOADING: &str = "Loading…";
const MANAGED_BY: &str = "managed by ";
const SEPARATOR: &str = " · ";
const LIST_SEPARATOR: &str = " · ";
const MORE: &str = "more · Open list →";
const GONE: &str = "is not on this cluster any more.";
const CLOSED: [&str; 3] = ["Pods", "ReplicaSets", "ConfigMaps"];

pub struct NamespaceScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    target: ResourceRef,
    menu: Entity<ActionMenu>,
    page: NamespacePage,
    preferences: Preferences,
    loaded: bool,
    toggled: BTreeSet<String>,
    filter: Entity<TextFieldState>,
    focus: FocusHandle,
    _filter: Subscription,
}

impl EventEmitter<Command> for NamespaceScreen {}
impl EventEmitter<ScreenEvent> for NamespaceScreen {}

impl Focusable for NamespaceScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn status_color(palette: &Palette, status: Status) -> gpui::Hsla {
    match status {
        Status::Ok => palette.fg2,
        other => palette.status(other),
    }
}

fn is_open(title: &str, toggled: &BTreeSet<String>, filtering: bool) -> bool {
    filtering || CLOSED.contains(&title) == toggled.contains(title)
}

fn row_selector(prefix: &str, ix: usize) -> String {
    format!("{prefix}-row-{ix}")
}

fn tile_selector(tile: &Tile) -> String {
    format!("namespace-tile-{}", tile.page.label()).to_lowercase()
}

fn weight(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

impl NamespaceScreen {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextFieldState::new(window, cx).placeholder(FILTER));
        let subscription = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        let screen = cx.weak_entity();
        let edit = edit_item(target.clone(), move |command, cx| {
            let _ = screen.update(cx, |_, cx| cx.emit(command));
        });
        let menu = cx.new(|cx| ActionMenu::new(ACTIONS, cx).items(vec![edit]));
        Self {
            navigation: None,
            target,
            menu,
            page: NamespacePage::default(),
            preferences: Preferences::default(),
            loaded: false,
            toggled: BTreeSet::new(),
            filter,
            focus: cx.focus_handle(),
            _filter: subscription,
        }
    }

    pub fn target(&self) -> &ResourceRef {
        &self.target
    }

    pub fn set(&mut self, snapshot: &Snapshot, preferences: &Preferences, cx: &mut Context<Self>) {
        let detail = snapshot.detail(&self.target.cluster);
        let page = NamespacePage::build(&self.target, detail, &snapshot.home.problems);
        let loaded = detail.is_some();
        let unchanged =
            self.page == page && self.loaded == loaded && &self.preferences == preferences;
        if unchanged {
            return;
        }
        self.page = page;
        self.loaded = loaded;
        self.preferences = preferences.clone();
        cx.notify();
    }

    fn open_list(&mut self, page: Page, list: &str, cx: &mut Context<Self>) {
        let selection = NamespaceSelection::Named(self.target.name.clone());
        self.preferences.select_namespace(list, selection);
        let cluster = self.target.cluster.clone();
        cx.emit(Command::SetPreferences(cluster, self.preferences.clone()));
        cx.emit(Command::SelectPage(page));
    }

    fn open_event(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(event) = self.page.recent_events().get(ix) else {
            return;
        };
        let key = event.key();
        let selection = NamespaceSelection::Named(self.target.name.clone());
        self.preferences.select_namespace(EVENT_LIST, selection);
        let cluster = self.target.cluster.clone();
        cx.emit(Command::SetPreferences(cluster, self.preferences.clone()));
        cx.emit(ScreenEvent::ShowEvent(key));
    }

    fn edit(&mut self, cx: &mut Context<Self>) {
        cx.emit(edit_command(self.target.clone()));
    }

    fn toggle(&mut self, title: &str, cx: &mut Context<Self>) {
        if !self.toggled.remove(title) {
            self.toggled.insert(title.to_owned());
        }
        cx.notify();
    }

    fn header(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        PageHeader::new(self.target.name.clone())
            .leading(self.navigation.clone())
            .kicker(SHORT_KICKER)
            .mono()
            .action(self.menu.clone())
            .meta(self.meta(palette, cx))
    }

    fn meta(&self, palette: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let created = self.page.meta();
        let owner = self.page.managed_by.clone().map(|owner| {
            let label = format!("{}/{}", short_kind(&owner.kind), owner.name);
            let link = Link::new("namespace-managed-by", label)
                .mono()
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(Command::OpenResource(owner.clone()));
                }));
            let separator = created.is_some().then_some(SEPARATOR);
            div()
                .flex()
                .child(MANAGED_BY)
                .child(link)
                .children(separator)
        });
        div()
            .flex()
            .flex_wrap()
            .text_color(palette.mute)
            .children(owner)
            .children(created)
    }

    fn drill(&self, page: Page, cx: &mut Context<Self>) -> Activate {
        let this = cx.weak_entity();
        let namespace = Some(self.target.name.clone());
        Rc::new(move |_, cx| {
            let namespace = namespace.clone();
            let event = ScreenEvent::NeedsAttention { page, namespace };
            let _ = this.update(cx, |_, cx| cx.emit(event));
        })
    }

    fn tile(&self, tile: &Tile, palette: &Palette, cx: &mut Context<Self>) -> HealthTile {
        let id = tile_selector(tile);
        let segments = tile
            .parts
            .iter()
            .map(|part| (part.status, weight(part.count)))
            .collect();
        let notes = tile
            .parts
            .iter()
            .map(|part| {
                if part.status == Status::Ok {
                    return TileNote::new(part.label.clone(), palette.mute);
                }
                let note = TileNote::new(part.label.clone(), palette.status(part.status));
                let drill = format!("{id}-{}", part.label.replace(' ', "-"));
                note.drill(drill, self.drill(tile.page, cx))
            })
            .collect();
        let page = tile.page;
        let list = namespace_list(page).unwrap_or(EVENT_LIST);
        let this = cx.weak_entity();
        HealthTile::new(
            id,
            page.label(),
            tile.total.to_string(),
            StackedBar::new(segments),
        )
        .notes(notes)
        .on_activate(move |_, cx| {
            let _ = this.update(cx, |this, cx| this.open_list(page, list, cx));
        })
    }

    fn tiles(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let tiles = self
            .page
            .tiles()
            .iter()
            .map(|tile| self.tile(tile, palette, cx))
            .collect();
        HealthTiles::new(tiles).into_any_element()
    }

    fn attention(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .page
            .attention
            .iter()
            .enumerate()
            .map(|(ix, problem)| attention_row(ix, problem, palette, cx))
            .collect();
        let body: AnyElement = if rows.is_empty() {
            div()
                .px(SPACE_8)
                .py(SPACE_5)
                .text_size(TEXT_BODY)
                .text_color(palette.mute)
                .child(NOTHING)
                .into_any_element()
        } else {
            div().flex().flex_col().children(rows).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .child(section_title(NEEDS_ATTENTION, None))
            .child(div().border_t(RULE).border_color(palette.line).child(body))
            .into_any_element()
    }

    fn contents(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.filter.read(cx).value().to_string();
        let filtering = !query.trim().is_empty();
        let groups = self.page.contents(&query);
        let field = div()
            .w(NAMESPACE_FILTER_WIDTH)
            .child(TextField::new(&self.filter).small());
        let title = section_title(CONTENTS, Some(self.page.objects())).child(field);
        let mut body: Vec<AnyElement> = groups
            .iter()
            .map(|group| self.group(group, filtering, palette, cx))
            .collect();
        if body.is_empty() && filtering {
            body.push(
                div()
                    .px(SPACE_8)
                    .py(SPACE_5)
                    .text_size(TEXT_BODY)
                    .text_color(palette.mute)
                    .child(NO_MATCH)
                    .into_any_element(),
            );
        }
        let empty = self.page.empty();
        let empty = (!empty.is_empty() && !filtering).then(|| {
            div()
                .px(SPACE_8)
                .pt(SPACE_5)
                .pb(SPACE_8)
                .text_size(TEXT_MONO)
                .text_color(palette.mute)
                .child(format!("{EMPTY}{}", empty.join(LIST_SEPARATOR)))
        });
        div()
            .flex()
            .flex_col()
            .child(title)
            .child(
                div()
                    .border_t(RULE)
                    .border_color(palette.line)
                    .flex()
                    .flex_col()
                    .children(body),
            )
            .children(empty)
            .into_any_element()
    }

    fn group(
        &self,
        group: &Group,
        filtering: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = is_open(&group.title, &self.toggled, filtering);
        let (status, summary) = self.page.summary(group);
        let title = group.title.clone();
        let selector = format!("namespace-group-{}", group.title);
        let chevron = if open { CHEVRON_DOWN } else { CHEVRON_RIGHT };
        let toggle = div()
            .debug_selector(move || selector)
            .id(gpui::SharedString::from(format!(
                "namespace-group-{}",
                group.title
            )))
            .flex()
            .items_center()
            .gap(SPACE_5)
            .h(ROW_TABLE_MAX)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&title, cx)))
            .child(svg().path(chevron).size(ICON).text_color(palette.mute))
            .child(
                div()
                    .w(NAMESPACE_CONTENT_NAME)
                    .flex_none()
                    .flex()
                    .items_baseline()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(group.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(TEXT_SMALL)
                            .text_color(palette.mute)
                            .child(group.total.to_string()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(TEXT_MONO)
                    .text_color(if status == Status::Ok {
                        palette.mute
                    } else {
                        palette.status(status)
                    })
                    .child(summary),
            );
        let rows: Vec<AnyElement> = if open {
            let mut rows: Vec<AnyElement> = group
                .items
                .iter()
                .enumerate()
                .map(|(ix, item)| content_row(&group.title, ix, item, palette, cx))
                .collect();
            let hidden = group.hidden();
            if hidden > 0
                && let Some(list) = group.list.clone()
            {
                let link = Link::new(
                    gpui::SharedString::from(format!("namespace-more-{}", group.title)),
                    format!("+{hidden} {MORE}"),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let key = flock_core::related::page_kind(list.page).unwrap_or(EVENT_LIST);
                    this.open_list(list.page, key, cx);
                }));
                rows.push(
                    div()
                        .flex()
                        .items_center()
                        .h(ROW_TABLE)
                        .px(SPACE_8)
                        .border_b(RULE)
                        .border_color(palette.line)
                        .text_size(TEXT_SMALL)
                        .child(link)
                        .into_any_element(),
                );
            }
            rows
        } else {
            Vec::new()
        };
        div()
            .flex()
            .flex_col()
            .child(toggle)
            .children(rows)
            .into_any_element()
    }

    fn aside(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let detail = self.page.detail.as_ref();
        let cpu = self.page.cpu();
        let memory = self.page.memory();
        let usage = detail.map(|detail| {
            let quota = fallback(&detail.quotas, palette).unwrap_or_else(|| {
                let lines = detail.quotas.items.iter().map(Quota::line);
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .children(lines)
                    .into_any_element()
            });
            div()
                .flex()
                .flex_col()
                .gap(SPACE_4)
                .px(SPACE_6)
                .pb(SPACE_6)
                .child(usage_row(CPU, &cpu, &cpu.cpu_label(), palette))
                .child(usage_row(MEMORY, &memory, &memory.memory_label(), palette))
                .child(aside_row(QUOTA, quota, palette))
        });
        let labels = self.page.namespace.as_ref().map(|namespace| {
            let chips: Vec<AnyElement> = namespace
                .labels
                .iter()
                .map(|(key, value)| chip(&format!("{key}={value}"), palette))
                .collect();
            div()
                .flex()
                .flex_wrap()
                .gap(SPACE_2)
                .px(SPACE_6)
                .pb(SPACE_6)
                .when(chips.is_empty(), |row| {
                    row.text_color(palette.mute).child(NONE)
                })
                .children(chips)
        });
        let policies = detail.map(|detail| {
            let lines = fallback(&detail.policies, palette)
                .unwrap_or_else(|| policy_lines(&detail.policies.items, palette));
            div()
                .px(SPACE_6)
                .pb(SPACE_6)
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .text_color(palette.fg2)
                .child(lines)
        });
        let events = self.events(palette, cx);
        div()
            .id("namespace-aside")
            .size_full()
            .overflow_y_scroll()
            .border_l(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .when_some(usage, |aside, usage| {
                aside.child(block(palette).child(aside_label(USAGE)).child(usage))
            })
            .when_some(labels, |aside, labels| {
                aside.child(block(palette).child(aside_label(LABELS)).child(labels))
            })
            .when_some(policies, |aside, policies| {
                aside.child(block(palette).child(aside_label(POLICIES)).child(policies))
            })
            .child(events)
            .into_any_element()
    }

    fn events(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let total = self.page.events.len();
        let all = Link::new("namespace-all-events", format!("{ALL} {total}"))
            .on_click(cx.listener(|this, _, _, cx| this.open_list(Page::Events, EVENT_LIST, cx)));
        let rows: Vec<AnyElement> = self
            .page
            .recent_events()
            .iter()
            .enumerate()
            .map(|(ix, event)| event_row(ix, event, palette, cx))
            .collect();
        let title = div()
            .flex()
            .items_center()
            .px(SPACE_6)
            .pt(SPACE_7)
            .pb(SPACE_3)
            .child(SectionLabel::new(EVENTS))
            .child(div().flex_1())
            .when(total > 0, |row| row.child(all));
        div()
            .flex()
            .flex_col()
            .child(title)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_3)
                    .px(SPACE_6)
                    .pb(SPACE_7)
                    .when(rows.is_empty(), |list| {
                        list.text_color(palette.mute).child(NONE)
                    })
                    .children(rows),
            )
            .into_any_element()
    }
}

fn section_title(title: &'static str, count: Option<String>) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(SPACE_5)
        .px(SPACE_8)
        .pt(SPACE_8)
        .pb(SPACE_3)
        .child(SectionLabel::new(title))
        .when_some(count, |row, count| {
            row.child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(TEXT_SMALL)
                    .child(count),
            )
        })
        .child(div().flex_1())
}

fn attention_row(
    ix: usize,
    problem: &Problem,
    palette: &Palette,
    cx: &mut Context<NamespaceScreen>,
) -> AnyElement {
    let target = problem.target.clone();
    let selector = row_selector("namespace-attention", ix);
    div()
        .debug_selector(move || selector)
        .id(("namespace-attention", ix))
        .flex()
        .items_center()
        .gap(SPACE_6)
        .h(ROW_TABLE_MAX)
        .px(SPACE_8)
        .border_b(RULE)
        .border_color(palette.line)
        .text_size(TEXT_BODY)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.emit(Command::OpenResource(target.clone()));
        }))
        .child(StatusGlyph::new(problem.status))
        .child(
            div()
                .w(NAMESPACE_KIND_WIDTH)
                .flex_none()
                .truncate()
                .text_color(palette.mute)
                .child(problem.target.kind.clone()),
        )
        .child(
            div()
                .w(NAMESPACE_NAME_WIDTH)
                .flex_none()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .child(problem.target.name.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(palette.fg2)
                .child(problem.message.clone()),
        )
        .child(
            div()
                .w(NAMESPACE_AGE_WIDTH)
                .flex_none()
                .flex()
                .justify_end()
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(age(problem.age)),
        )
        .into_any_element()
}

fn content_row(
    group: &str,
    ix: usize,
    item: &Item,
    palette: &Palette,
    cx: &mut Context<NamespaceScreen>,
) -> AnyElement {
    let target = item.target.clone();
    let linked = item.linked;
    let selector = row_selector(&format!("namespace-{group}"), ix);
    let status = item.status.unwrap_or_default();
    div()
        .debug_selector(move || selector)
        .id((gpui::SharedString::from(format!("namespace-{group}")), ix))
        .flex()
        .items_center()
        .gap(SPACE_5)
        .h(ROW_TABLE)
        .px(SPACE_8)
        .border_b(RULE)
        .border_color(palette.line)
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .when(linked, |row| {
            row.cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(Command::OpenResource(target.clone()));
                }))
        })
        .child(div().w(ICON).flex_none())
        .child(
            div()
                .w(NAMESPACE_CONTENT_NAME)
                .flex_none()
                .truncate()
                .child(item.label.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(TEXT_SMALL)
                .text_color(if status == Status::Ok {
                    palette.mute
                } else {
                    palette.status(status)
                })
                .child(item.note.clone()),
        )
        .child(
            div()
                .w(NAMESPACE_CONTENT_AGE)
                .flex_none()
                .flex()
                .justify_end()
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .children(item.age.map(age)),
        )
        .into_any_element()
}

fn block(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .border_b(RULE)
        .border_color(palette.line)
}

fn aside_label(text: &'static str) -> impl IntoElement {
    div()
        .px(SPACE_6)
        .pt(SPACE_7)
        .pb(SPACE_3)
        .child(SectionLabel::new(text))
}

fn aside_row(label: &'static str, value: impl IntoElement, palette: &Palette) -> impl IntoElement {
    div()
        .flex()
        .gap(SPACE_5)
        .child(
            div()
                .w(NAMESPACE_USAGE_LABEL)
                .flex_none()
                .text_color(palette.mute)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(FONT_MONO)
                .text_size(TEXT_MONO)
                .child(value),
        )
}

fn usage_row(
    label: &'static str,
    requested: &Requested,
    text: &str,
    palette: &Palette,
) -> impl IntoElement {
    let bar = requested.share().map(|share| {
        div()
            .h(NAMESPACE_USAGE_BAR)
            .w_full()
            .flex()
            .bg(palette.line2)
            .child(div().h_full().w(relative(share)).bg(palette.fg2))
    });
    let value = div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(text.to_owned())
        .children(bar);
    aside_row(label, value, palette)
}

fn fallback<T>(listing: &Listing<T>, palette: &Palette) -> Option<AnyElement> {
    let text = match listing.unavailable() {
        Some(reason) => reason.to_owned(),
        None if listing.items.is_empty() && listing.loading() => LOADING.to_owned(),
        None if listing.items.is_empty() => NONE.to_owned(),
        None => return None,
    };
    let note = div().text_color(palette.mute).child(text);
    Some(note.into_any_element())
}

fn policy_lines(policies: &[Policy], palette: &Palette) -> AnyElement {
    let rows = policies.iter().map(|policy| {
        div()
            .flex()
            .gap(SPACE_2)
            .child(div().flex_none().child(policy.name.clone()))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(palette.mute)
                    .child(format!("{}{}", SEPARATOR.trim_start(), policy.summary)),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .children(rows)
        .into_any_element()
}

fn chip(text: &str, palette: &Palette) -> AnyElement {
    div()
        .px(SPACE_3)
        .py(SPACE_1)
        .rounded(RADIUS)
        .bg(palette.raise)
        .font_family(FONT_MONO)
        .text_size(TEXT_LABEL)
        .text_color(palette.fg2)
        .child(text.to_owned())
        .into_any_element()
}

fn event_row(
    ix: usize,
    event: &Event,
    palette: &Palette,
    cx: &mut Context<NamespaceScreen>,
) -> AnyElement {
    let message = div()
        .id(("namespace-event-message", ix))
        .debug_selector(move || format!("namespace-event-message-{ix}"))
        .text_color(palette.fg2)
        .line_clamp(NAMESPACE_EVENT_LINES)
        .text_ellipsis()
        .tooltip(Tooltip::text(event.message.clone()))
        .child(event.message.clone());
    div()
        .id(("namespace-event", ix))
        .debug_selector(move || format!("namespace-event-{ix}"))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| this.open_event(ix, cx)))
        .flex()
        .gap(SPACE_4)
        .text_size(TEXT_MONO)
        .line_height(relative(crate::theme::LINE_PROSE))
        .child(
            div()
                .w(NAMESPACE_EVENT_AGE)
                .flex_none()
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(age(event.age)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_color(status_color(palette, event.status))
                        .child(event.reason.clone()),
                )
                .child(message),
        )
        .into_any_element()
}

impl Render for NamespaceScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let root = div()
            .id("namespace")
            .key_context(YAML_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &EditYaml, _, cx| this.edit(cx)))
            .size_full()
            .flex()
            .bg(palette.editor);
        let missing = self.loaded && self.page.namespace.is_none() && self.page.groups.is_empty();
        let main = div()
            .id("namespace-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(self.header(&palette, cx));
        if missing {
            let sentence = format!("{KICKER} {} {GONE}", self.target.name);
            return root.child(main.child(EmptyState::new(sentence).selector("namespace-gone")));
        }
        if !self.loaded || self.page.groups.is_empty() {
            return root.child(main.child(Skeleton::rows()));
        }
        let main = main
            .child(self.tiles(&palette, cx))
            .child(self.attention(&palette, cx))
            .child(self.contents(&palette, cx))
            .child(div().h(SPACE_8));
        let aside =
            InspectorPane::new("namespace-inspector", self.aside(&palette, cx)).width(ASIDE_WIDTH);
        root.child(main).child(aside)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pods_replica_sets_and_config_maps_start_collapsed_and_a_filter_opens_everything() {
        let mut toggled = BTreeSet::new();
        assert!(!is_open("Pods", &toggled, false));
        assert!(is_open("Deployments", &toggled, false));
        assert!(is_open("Pods", &toggled, true));
        toggled.insert("Pods".to_owned());
        toggled.insert("Deployments".to_owned());
        assert!(is_open("Pods", &toggled, false));
        assert!(!is_open("Deployments", &toggled, false));
        assert_eq!(
            row_selector("namespace-attention", 0),
            "namespace-attention-row-0"
        );
    }
}
