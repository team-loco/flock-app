use crate::kit::{SelectableText, YamlRow};
use flock_core::flux::{
    FluxDetail, HelmDetail, HelmRevision, HistoryEntry, InventoryFilter, ManagedObject, Phase,
    SpecRow, peer_summary,
};
use flock_core::issue::Issue;
use flock_core::{Command, FluxAction, FluxKind, FluxObject, FluxPage, Page, Status, time};
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder, px,
};

use crate::kit::{
    ActionMenu, Button, CONDITIONS, CodeView, Column, ConditionTable, ConfirmDialog, DiffView,
    ErrorBlocks, InspectorPane, KeyValue, Link, MenuItem, PageHeader, PanelSection, Prose,
    SectionHeader, Segmented, StatusGlyph, Table, TableEvent, Toast, confirm, issue_stack,
    parse_unified, toast,
};
use crate::screens::edit::{EDIT_KEYS, EditYaml, YAML_CONTEXT, edit_command};
use crate::screens::{EDIT_DESCRIPTION, EDIT_YAML};
use crate::screens::{FullView, full_view_link};
use crate::theme::{
    ASIDE_WIDTH, ActiveTheme, FONT_MONO, FONT_SANS, Palette, ROW_DOCUMENT, ROW_TABLE_MIN, RULE,
    SPACE_2, SPACE_3, SPACE_5, SPACE_8, TEXT_BODY, TEXT_MONO, TEXT_SMALL,
};

pub(crate) const FLUX_CONTEXT: &str = "FluxObject";
const GLYPH_COLUMN: Pixels = px(14.);
const KIND_COLUMN: Pixels = px(170.);
const NAME_MIN: Pixels = px(200.);
const DETAIL_MIN: Pixels = px(160.);
const NAME_WEIGHT: f32 = 3.;
const DETAIL_WEIGHT: f32 = 1.;
const PHRASE_SEPARATOR: &str = " · ";
const PHRASE_BREAK: &str = " ·\u{a0}";
const AGE_COLUMN: Pixels = px(40.);
const REVISION_COLUMN: Pixels = px(70.);
const COUNT_COLUMN: Pixels = px(72.);
const COUNT_MAX: Pixels = px(90.);
const REVISION_MAX: Pixels = px(160.);
const PEER_GLYPH: Pixels = px(12.);
const PEER_CLUSTER: Pixels = px(96.);
const PEER_REVISION: Pixels = px(64.);
const SPEC_LABEL: Pixels = px(64.);
const VALUES_LABEL: Pixels = px(140.);

const NO_VALUES: &str = "No values set.";
const RECONCILE: &str = "Reconcile";
const RESUME: &str = "Resume";
const RECONCILE_KEYS: &str = "⌘R";
const WITH_SOURCE_KEYS: &str = "⇧⌘R";
const DIFF_KEYS: &str = "⌘D";
const YAML_KEYS: &str = "⌘Y";
const COMPARE: &str = "Compare →";
const ASIDE_ID: &str = "flux-inspector";
const OTHER_CLUSTERS: &str = "Other clusters";
const DEPENDS_ON: &str = "Depends on";
const VALUES: &str = "Values";
const SPEC: &str = "Spec";
const READ_ONLY: &str = "read-only";

gpui::actions!(
    flux_object,
    [Reconcile, ReconcileWithSource, DiffAgainstGit, ViewYaml]
);

pub struct FluxObjectPage {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    page: Option<FluxPage>,
    filter: InventoryFilter,
    yaml_open: bool,
    release: Option<u32>,
    focus: FocusHandle,
    wants_focus: bool,
    menu: Entity<ActionMenu>,
    inventory: Entity<Table<ManagedObject>>,
    history: Entity<Table<HistoryEntry>>,
    releases: Entity<Table<HelmRevision>>,
    dependents: Entity<Table<FluxObject>>,
    compact: Option<FullView>,
    issues: Vec<Issue>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Command> for FluxObjectPage {}

impl gpui::Focusable for FluxObjectPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FluxObjectPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let menu = cx.new(|cx| ActionMenu::new("flux-actions", cx));
        let inventory = cx.new(|cx| {
            Table::new("flux-inventory", inventory_columns(), cx)
                .headerless()
                .click_opens()
                .row_height(ROW_DOCUMENT)
                .row_key(|o: &ManagedObject| format!("{}/{}", o.kind, o.path()).into())
                .empty("Nothing matches this filter.")
                .fit_content()
        });
        let history = cx.new(|cx| {
            Table::new("flux-history", history_columns(), cx)
                .headerless()
                .row_height(ROW_DOCUMENT)
                .empty("No reconciles recorded yet.")
                .fit_content()
        });
        let releases = cx.new(|cx| {
            Table::new("flux-releases", release_columns(), cx)
                .headerless()
                .row_height(ROW_DOCUMENT)
                .row_key(|r: &HelmRevision| r.revision.to_string().into())
                .empty("Helm has no releases stored for this object.")
                .fit_content()
        });
        let dependents = cx.new(|cx| {
            Table::new("flux-dependents", dependent_columns(), cx)
                .headerless()
                .click_opens()
                .row_height(ROW_DOCUMENT)
                .empty("Nothing on this cluster builds from this source.")
                .fit_content()
        });
        let subscriptions = vec![
            cx.subscribe(&inventory, |page, table, event, cx| {
                page.inventory_event(&table, event, cx);
            }),
            cx.subscribe(&releases, |page, table, event, cx| {
                page.release_event(&table, event, cx);
            }),
            cx.subscribe(&dependents, |_, table, event, cx| {
                Self::dependent_event(&table, event, cx);
            }),
        ];
        Self {
            navigation: None,
            page: None,
            filter: InventoryFilter::All,
            yaml_open: false,
            release: None,
            focus: cx.focus_handle(),
            wants_focus: false,
            menu,
            inventory,
            history,
            releases,
            dependents,
            compact: None,
            issues: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_compact(&mut self, compact: Option<FullView>, cx: &mut Context<Self>) {
        if self.compact.is_some() != compact.is_some() {
            cx.notify();
        }
        self.compact = compact;
    }

    fn compact_header(
        &self,
        object: &FluxObject,
        open: &FullView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        PageHeader::new(object.key.name.clone())
            .leading(self.navigation.clone())
            .mono()
            .status(object.status(), object.phase.label())
            .detail(issue_stack(self.issues.clone()))
            .meta(Self::meta(object, cx))
            .footer(full_view_link("flux-full-view", open))
            .into_any_element()
    }

    fn keep_issues(&mut self, opened: bool) {
        let Some(object) = self.object() else {
            self.issues.clear();
            return;
        };
        let fresh = object.issues();
        let reconciling = matches!(object.phase, Phase::Reconciling);
        if fresh.is_empty() && reconciling && !opened {
            return;
        }
        self.issues = fresh;
    }

    pub fn set_page(&mut self, page: Option<FluxPage>, cx: &mut Context<Self>) {
        let previous = self.page.as_ref().map(|p| p.object.key.clone());
        let next = page.as_ref().map(|p| p.object.key.clone());
        let opened = previous != next;
        if opened {
            self.filter = InventoryFilter::All;
            self.yaml_open = false;
            self.release = None;
            self.wants_focus = next.is_some();
        }
        self.page = page;
        self.keep_issues(opened);
        let Some(page) = self.page.clone() else {
            cx.notify();
            return;
        };
        if let FluxDetail::HelmRelease(detail) = &page.object.detail {
            let known = self
                .release
                .filter(|r| detail.releases.iter().any(|x| x.revision == *r));
            self.release = known.or_else(|| detail.first_change());
            let rows = detail.releases.clone();
            let selected = self
                .release
                .and_then(|r| rows.iter().position(|x| x.revision == r));
            self.releases.update(cx, |table, cx| {
                table.set_rows(rows, cx);
                table.select(selected, cx);
            });
        }
        self.refresh_inventory(cx);
        let history = page.object.history.clone();
        self.history
            .update(cx, |table, cx| table.set_rows(history, cx));
        let dependents = page.dependents.clone();
        self.dependents
            .update(cx, |table, cx| table.set_rows(dependents, cx));
        let items = Self::menu_items(&page, cx);
        self.menu.update(cx, |menu, cx| menu.set_items(items, cx));
        cx.notify();
    }

    fn object(&self) -> Option<&FluxObject> {
        self.page.as_ref().map(|page| &page.object)
    }

    fn refresh_inventory(&mut self, cx: &mut Context<Self>) {
        let rows = self
            .object()
            .map(|object| object.inventory(self.filter))
            .unwrap_or_default();
        let empty = self.object().map_or("", FluxObject::inventory_empty);
        self.inventory.update(cx, |table, cx| {
            table.set_empty(empty, cx);
            table.set_rows(rows, cx);
            table.select(None, cx);
        });
    }

    fn set_filter(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(filter) = InventoryFilter::ALL.get(ix).copied() else {
            return;
        };
        self.filter = filter;
        self.refresh_inventory(cx);
        cx.notify();
    }

    fn act(&mut self, action: FluxAction, cx: &mut Context<Self>) {
        let Some(object) = self.object() else {
            return;
        };
        let command = Command::Flux(object.key.clone(), object.uid.clone(), action);
        cx.emit(command);
    }

    fn toggle_yaml(&mut self, cx: &mut Context<Self>) {
        self.yaml_open = !self.yaml_open;
        cx.notify();
    }

    fn edit_yaml(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object() else {
            return;
        };
        let target = object.key.resource();
        cx.emit(edit_command(target));
    }

    fn copy_yaml(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object() else {
            return;
        };
        let item = ClipboardItem::new_string(object.yaml.clone());
        let detail = object_detail(object);
        cx.write_to_clipboard(item);
        toast(Toast::new(Status::Ok, "Copied YAML").detail(detail), cx);
    }

    fn inventory_event(
        &mut self,
        table: &Entity<Table<ManagedObject>>,
        event: &TableEvent,
        cx: &mut Context<Self>,
    ) {
        let TableEvent::Opened(ix) = event else {
            return;
        };
        let Some(cluster) = self.object().map(|object| object.key.cluster.clone()) else {
            return;
        };
        let target = table.read(cx).row(*ix).map(|row| row.resource(&cluster));
        if let Some(resource) = target {
            cx.emit(Command::OpenResource(resource));
        }
    }

    fn release_event(
        &mut self,
        table: &Entity<Table<HelmRevision>>,
        event: &TableEvent,
        cx: &mut Context<Self>,
    ) {
        let TableEvent::Selected(ix) = event else {
            return;
        };
        self.release = table.read(cx).row(*ix).map(|r| r.revision);
        cx.notify();
    }

    fn dependent_event(
        table: &Entity<Table<FluxObject>>,
        event: &TableEvent,
        cx: &mut Context<Self>,
    ) {
        let TableEvent::Opened(ix) = event else {
            return;
        };
        let target = table.read(cx).row(*ix).map(|row| row.key.clone());
        if let Some(key) = target {
            cx.emit(Command::OpenFluxObject(key));
        }
    }

    fn menu_items(page: &FluxPage, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let this = cx.weak_entity();
        let object = &page.object;
        let source = object.source.clone();
        let mut items = Vec::new();
        if let Some(label) = fetch_label(object.key.kind) {
            let fetch = item(&this, label, |page, _, cx| {
                page.act(FluxAction::ReconcileWithSource, cx);
            })
            .shortcut(WITH_SOURCE_KEYS);
            items.push(described(
                fetch,
                object.key.cli(FluxAction::ReconcileWithSource),
            ));
        }
        items.push(suspend_item(&this, object));
        if object.key.kind == FluxKind::Kustomization {
            items.push(
                item(&this, "Diff cluster against git", |_, _, cx| {
                    cx.emit(Command::SelectPage(Page::Changes));
                })
                .shortcut(DIFF_KEYS),
            );
        }
        items.push(
            item(&this, "View YAML", |page, _, cx| page.toggle_yaml(cx))
                .shortcut(YAML_KEYS)
                .description(READ_ONLY),
        );
        items.push(
            item(&this, EDIT_YAML, |page, _, cx| page.edit_yaml(cx))
                .shortcut(EDIT_KEYS)
                .description(EDIT_DESCRIPTION),
        );
        items.push(item(&this, "Copy YAML", |page, _, cx| page.copy_yaml(cx)));
        if let Some(source) = source {
            items.push(item(&this, "Open source", move |_, _, cx| {
                cx.emit(Command::OpenFluxObject(source.clone()));
            }));
        }
        if let Some(force) = force_item(&this, object) {
            items.push(force);
        }
        items
    }

    fn header(&self, object: &FluxObject, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.weak_entity();
        let primary = if object.phase.is_suspended() {
            Button::new("flux-resume", RESUME).on_click(move |_, _, cx| {
                this.update(cx, |page, cx| page.act(FluxAction::Resume, cx))
                    .ok();
            })
        } else {
            Button::new("flux-reconcile", RECONCILE)
                .shortcut(RECONCILE_KEYS)
                .on_click(move |_, _, cx| {
                    this.update(cx, |page, cx| page.act(FluxAction::Reconcile, cx))
                        .ok();
                })
        };
        PageHeader::new(object.key.name.clone())
            .leading(self.navigation.clone())
            .mono()
            .status(object.status(), object.phase.label())
            .action(primary.primary())
            .action(self.menu.clone())
            .meta(Self::meta(object, cx))
            .into_any_element()
    }

    fn meta(object: &FluxObject, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.weak_entity();
        let open_commit = move |sha: String| {
            let this = this.clone();
            move |_: &mut Window, cx: &mut App| {
                let command = Command::OpenCommit(sha.clone());
                this.update(cx, |_, cx| cx.emit(command)).ok();
            }
        };
        let mut prose = Prose::new("flux-meta");
        let mut has_text = false;
        match &object.detail {
            FluxDetail::Kustomization(_) => {}
            FluxDetail::HelmRelease(detail) => {
                prose = prose.text(format!("chart {}", detail.chart));
                has_text = true;
            }
            FluxDetail::Source(source) => {
                let fetched = time::age(source.fetched);
                if let Some(artifact) = &object.applied {
                    let git = object.key.kind == FluxKind::GitRepository;
                    let handler = open_commit(artifact.id.clone());
                    prose = revision_link(prose.text("artifact "), &artifact.id, git, handler)
                        .text(format!(" {} ago · ", time::age(artifact.age)));
                }
                return prose
                    .text(format!("last fetch {fetched} ago"))
                    .into_any_element();
            }
        }
        let commits = object.key.kind == FluxKind::Kustomization;
        if let Some(applied) = &object.applied {
            prose = prose.text(format!("{}applied ", separator(has_text)));
            prose = revision_link(prose, &applied.id, commits, open_commit(applied.id.clone()));
            prose = prose.text(format!(" {} ago", time::age(applied.age)));
            has_text = true;
        }
        if let Some(healthy) = object.shown_last_healthy() {
            prose = prose.text(format!("{}last healthy ", separator(has_text)));
            prose = revision_link(prose, &healthy.id, commits, open_commit(healthy.id.clone()));
            prose = prose.text(format!(" {} ago", time::age(healthy.age)));
        }
        prose.into_any_element()
    }

    fn inventory_section(
        &self,
        object: &FluxObject,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let applied = object.applied.as_ref();
        let selected = InventoryFilter::ALL
            .iter()
            .position(|f| *f == self.filter)
            .unwrap_or_default();
        let filters = InventoryFilter::ALL
            .iter()
            .fold(Segmented::new("inventory-filter"), |group, filter| {
                group.option(filter.label(applied))
            });
        let filters = filters
            .small()
            .selected(selected)
            .on_select(cx.listener(|page, ix: &usize, _, cx| page.set_filter(*ix, cx)));
        div()
            .child(
                SectionHeader::new("Managed objects")
                    .count(object.inventory.len().to_string())
                    .end(filters),
            )
            .child(ruled(Table::pane(&self.inventory, cx), palette))
            .into_any_element()
    }

    fn drift_section(
        object: &FluxObject,
        drift: Option<&flock_core::flux::Drift>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.weak_entity();
        let open: super::flux_drift::OpenResource = std::rc::Rc::new(move |target, _, cx| {
            this.update(cx, |_, cx| cx.emit(Command::OpenResource(target)))
                .ok();
        });
        super::flux_drift::section(drift, &object.key.cluster, &open, palette)
    }

    fn diagnosis_section(object: &FluxObject) -> AnyElement {
        let conditions = (!object.conditions.is_empty()).then(|| {
            div().child(SectionHeader::new(CONDITIONS)).child(
                div()
                    .px(SPACE_8)
                    .pb(SPACE_5)
                    .child(ConditionTable::new(&object.conditions)),
            )
        });
        div().children(conditions).into_any_element()
    }

    fn history_section(&self, palette: &Palette, cx: &App) -> AnyElement {
        div()
            .child(SectionHeader::new("History"))
            .child(ruled(Table::pane(&self.history, cx), palette))
            .into_any_element()
    }

    fn dependents_section(&self, page: &FluxPage, palette: &Palette, cx: &App) -> AnyElement {
        div()
            .child(SectionHeader::new("Used by").count(page.dependents.len().to_string()))
            .child(ruled(Table::pane(&self.dependents, cx), palette))
            .into_any_element()
    }

    fn releases_section(&self, detail: &HelmDetail, palette: &Palette, cx: &App) -> AnyElement {
        let count = format!("{} · select one to see what changed", detail.releases.len());
        let selected = self
            .release
            .and_then(|r| detail.releases.iter().find(|x| x.revision == r));
        div()
            .child(SectionHeader::new("History").count(count))
            .child(ruled(Table::pane(&self.releases, cx), palette))
            .children(selected.map(|release| revision_diff(detail, release, palette)))
            .into_any_element()
    }

    fn yaml_section(object: &FluxObject, cx: &mut Context<Self>) -> AnyElement {
        let edit = Button::new("flux-edit-yaml", EDIT_YAML)
            .small()
            .tooltip(EDIT_KEYS)
            .on_click(cx.listener(|page, _, _, cx| page.edit_yaml(cx)));
        div()
            .child(SectionHeader::new("YAML").end(edit))
            .child(
                div()
                    .px(SPACE_8)
                    .child(CodeView::new("flux-yaml", object.yaml.clone()).yaml()),
            )
            .into_any_element()
    }

    fn main(&self, page: &FluxPage, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let object = &page.object;
        let body: Vec<AnyElement> = match &object.detail {
            FluxDetail::Kustomization(drift) => vec![
                self.inventory_section(object, palette, cx),
                Self::drift_section(object, drift.as_ref(), palette, cx),
                self.history_section(palette, cx),
            ],
            FluxDetail::HelmRelease(detail) => vec![
                self.inventory_section(object, palette, cx),
                self.releases_section(detail, palette, cx),
            ],
            FluxDetail::Source(_) => vec![
                self.dependents_section(page, palette, cx),
                self.history_section(palette, cx),
            ],
        };
        div()
            .id("flux-main")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(self.header(object, cx))
            .child(Self::diagnosis_section(object))
            .children(body)
            .when(self.yaml_open, |main| {
                main.child(Self::yaml_section(object, cx))
            })
            .child(div().h(SPACE_8))
            .into_any_element()
    }

    fn aside(&self, page: &FluxPage, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let object = &page.object;
        let this = cx.weak_entity();
        let peers: Vec<AnyElement> = page
            .peers
            .iter()
            .enumerate()
            .map(|(ix, peer)| peer_row(ix, peer, &this, palette))
            .collect();
        let comparing = this.clone();
        let compare = Link::new("flux-compare", COMPARE).on_click(move |_, _, cx| {
            comparing
                .update(cx, |_, cx| {
                    cx.emit(Command::SelectPage(Page::Changes));
                })
                .ok();
        });
        let dependencies: Vec<AnyElement> = page
            .dependencies
            .iter()
            .enumerate()
            .map(|(ix, dependency)| {
                let key = dependency.key.clone();
                let this = this.clone();
                div()
                    .id(("flux-dependency", ix))
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.acc))
                    .on_click(move |_, _, cx| {
                        this.update(cx, |_, cx| cx.emit(Command::OpenFluxObject(key.clone())))
                            .ok();
                    })
                    .child(StatusGlyph::new(dependency.status))
                    .child(dependency.key.name.clone())
                    .into_any_element()
            })
            .collect();
        let values = values_block(object, palette);
        let peers = (!peers.is_empty()).then(|| {
            PanelSection::new(ASIDE_ID, OTHER_CLUSTERS)
                .count(peer_summary(&page.peers))
                .trailing(compare)
                .child(div().flex().flex_col().children(peers))
        });
        let dependencies = (!dependencies.is_empty()).then(|| {
            PanelSection::new(ASIDE_ID, DEPENDS_ON)
                .count(dependencies.len().to_string())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACE_2)
                        .font_family(FONT_MONO)
                        .text_size(TEXT_MONO)
                        .children(dependencies),
                )
        });
        let values = values.map(|values| PanelSection::new(ASIDE_ID, VALUES).child(values));
        div()
            .id("flux-aside")
            .w_full()
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_l(RULE)
            .border_color(palette.line)
            .bg(palette.panel)
            .text_size(TEXT_BODY)
            .child(ErrorBlocks::new(self.issues.clone()))
            .children(peers)
            .children(dependencies)
            .children(values)
            .child(PanelSection::new(ASIDE_ID, SPEC).child(spec_list(&object.spec, SPEC_LABEL)))
            .child(YamlRow::new(ASIDE_ID, move |_, cx| {
                this.update(cx, FluxObjectPage::toggle_yaml).ok();
            }))
            .into_any_element()
    }
}

impl Render for FluxObjectPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        if std::mem::take(&mut self.wants_focus) {
            window.focus(&self.focus, cx);
        }
        let compact = self.compact.clone();
        let pending = self.page.is_none().then(|| {
            crate::kit::PageTitle::new(Page::GitOps.title()).leading(self.navigation.clone())
        });
        let content = self.page.clone().map(|page| {
            if let Some(open) = &compact {
                return div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(self.compact_header(&page.object, open, cx));
            }
            let aside = self.aside(&page, &palette, cx);
            let aside = InspectorPane::new(ASIDE_ID, aside).width(ASIDE_WIDTH);
            div()
                .flex()
                .size_full()
                .child(self.main(&page, &palette, cx))
                .child(aside)
        });
        let key_context = if self.yaml_open {
            format!("{FLUX_CONTEXT} {YAML_CONTEXT}")
        } else {
            FLUX_CONTEXT.to_owned()
        };
        div()
            .id("flux-object")
            .key_context(key_context.as_str())
            .track_focus(&self.focus)
            .on_action(cx.listener(|page, _: &Reconcile, _, cx| {
                let suspended = page.object().is_some_and(|o| o.phase.is_suspended());
                if !suspended {
                    page.act(FluxAction::Reconcile, cx);
                }
            }))
            .on_action(cx.listener(|page, _: &ReconcileWithSource, _, cx| {
                page.act(FluxAction::ReconcileWithSource, cx);
            }))
            .on_action(cx.listener(|_, _: &DiffAgainstGit, _, cx| {
                cx.emit(Command::SelectPage(Page::Changes));
            }))
            .on_action(cx.listener(|page, _: &ViewYaml, _, cx| page.toggle_yaml(cx)))
            .on_action(cx.listener(|page, _: &EditYaml, _, cx| page.edit_yaml(cx)))
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(palette.editor)
            .font_family(FONT_SANS)
            .flex()
            .flex_col()
            .children(pending)
            .children(content)
    }
}

fn item(
    this: &WeakEntity<FluxObjectPage>,
    label: &str,
    run: impl Fn(&mut FluxObjectPage, &mut Window, &mut Context<FluxObjectPage>) + 'static,
) -> MenuItem {
    let this = this.clone();
    MenuItem::new(label.to_owned(), move |window, cx| {
        this.update(cx, |page, cx| run(page, window, cx)).ok();
    })
}

fn described(item: MenuItem, description: Option<String>) -> MenuItem {
    match description {
        Some(description) => item.description(description),
        None => item,
    }
}

fn suspend_item(this: &WeakEntity<FluxObjectPage>, object: &FluxObject) -> MenuItem {
    if object.phase.is_suspended() {
        let resume = item(this, RESUME, |page, _, cx| {
            page.act(FluxAction::Resume, cx);
        });
        described(resume, object.key.cli(FluxAction::Resume))
    } else {
        let suspend = item(this, "Suspend", |page, _, cx| {
            page.act(FluxAction::Suspend, cx);
        });
        described(suspend, object.key.cli(FluxAction::Suspend))
    }
}

fn force_item(this: &WeakEntity<FluxObjectPage>, object: &FluxObject) -> Option<MenuItem> {
    let (label, body, confirm_label) = match object.key.kind {
        FluxKind::Kustomization => (
            "Force apply…",
            "Flock sets spec.force to true and requests a reconcile. Flux deletes and recreates every object whose immutable fields changed, and pods behind them restart. When Flux finishes, Flock restores spec.force. If Flock quits first, spec.force stays true.",
            "Force apply",
        ),
        FluxKind::HelmRelease => (
            "Force upgrade…",
            "Helm upgrades the release now even if nothing changed, and the retry count starts over. Pods may restart.",
            "Force upgrade",
        ),
        FluxKind::GitRepository => return None,
        FluxKind::HelmRepository => return None,
        FluxKind::OciRepository => return None,
    };
    let title = format!("{} {}", confirm_label, object.key.name);
    let detail = object_detail(object);
    let weak = this.clone();
    let open =
        move |_: &mut FluxObjectPage, window: &mut Window, cx: &mut Context<FluxObjectPage>| {
            let weak = weak.clone();
            let dialog = ConfirmDialog::new(title.clone(), body, confirm_label, move |_, cx| {
                weak.update(cx, |page, cx| page.act(FluxAction::Force, cx))
                    .ok();
            })
            .detail(detail.clone());
            confirm(dialog, window, cx);
        };
    let force = item(this, label, open)
        .destructive()
        .disabled(object.phase.is_suspended());
    Some(described(force, object.key.cli(FluxAction::Force)))
}

fn fetch_label(kind: FluxKind) -> Option<&'static str> {
    match kind {
        FluxKind::Kustomization => Some("Fetch git, then reconcile"),
        FluxKind::HelmRelease => Some("Fetch chart, then reconcile"),
        FluxKind::GitRepository => None,
        FluxKind::HelmRepository => None,
        FluxKind::OciRepository => None,
    }
}

fn object_detail(object: &FluxObject) -> String {
    format!(
        "{} · {}/{}",
        object.key.cluster, object.key.namespace, object.key.name
    )
}

fn revision_link(
    prose: Prose,
    id: &str,
    linked: bool,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> Prose {
    if linked {
        prose.link(id.to_owned(), handler)
    } else {
        prose.text(id.to_owned())
    }
}

fn ruled(table: impl IntoElement, palette: &Palette) -> impl IntoElement {
    div().border_t(RULE).border_color(palette.line).child(table)
}

fn wrappable(value: &str) -> String {
    let phrases: Vec<String> = value
        .split(PHRASE_SEPARATOR)
        .map(|phrase| phrase.replace(' ', "\u{a0}"))
        .collect();
    phrases.join(PHRASE_BREAK)
}

fn spec_value(value: &str) -> AnyElement {
    SelectableText::new(wrappable(value)).into_any_element()
}

fn values_block(object: &FluxObject, palette: &Palette) -> Option<AnyElement> {
    match &object.detail {
        FluxDetail::HelmRelease(detail) if detail.values.is_empty() => Some(no_values(palette)),
        FluxDetail::HelmRelease(detail) => {
            Some(spec_list(&detail.values, VALUES_LABEL).into_any_element())
        }
        FluxDetail::Kustomization(_) => None,
        FluxDetail::Source(_) => None,
    }
}

fn no_values(palette: &Palette) -> AnyElement {
    div()
        .text_size(TEXT_SMALL)
        .text_color(palette.mute)
        .child(NO_VALUES)
        .into_any_element()
}

fn spec_list(rows: &[SpecRow], label_width: Pixels) -> impl IntoElement {
    let list = rows.iter().fold(KeyValue::new(), |list, row| {
        list.row(row.label.clone(), spec_value(&row.value))
    });
    div()
        .font_family(FONT_MONO)
        .text_size(TEXT_SMALL)
        .child(list.label_width(label_width))
}

fn peer_row(
    ix: usize,
    peer: &FluxObject,
    this: &WeakEntity<FluxObjectPage>,
    palette: &Palette,
) -> AnyElement {
    let key = peer.key.clone();
    let this = this.clone();
    let revision = peer
        .applied
        .as_ref()
        .map(|r| r.id.clone())
        .unwrap_or_default();
    let hover = palette.hover;
    div()
        .id(("flux-peer", ix))
        .flex()
        .items_center()
        .gap(SPACE_3)
        .h(ROW_TABLE_MIN)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, _, cx| {
            this.update(cx, |_, cx| cx.emit(Command::OpenFluxObject(key.clone())))
                .ok();
        })
        .font_family(FONT_MONO)
        .text_size(TEXT_MONO)
        .child(
            div()
                .w(PEER_GLYPH)
                .flex_none()
                .child(StatusGlyph::new(peer.status())),
        )
        .child(
            div()
                .w(PEER_CLUSTER)
                .flex_none()
                .truncate()
                .font_family(FONT_SANS)
                .text_size(TEXT_BODY)
                .child(peer.key.cluster.to_string()),
        )
        .child(
            div()
                .w(PEER_REVISION)
                .flex_none()
                .truncate()
                .text_color(palette.acc)
                .child(revision),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(palette.mute)
                .child(peer.phase.note()),
        )
        .into_any_element()
}

fn revision_diff(detail: &HelmDetail, release: &HelmRevision, palette: &Palette) -> AnyElement {
    let previous = detail.previous(release.revision);
    let title = match previous {
        Some(previous) => format!(
            "Revision {} against {}",
            release.revision, previous.revision
        ),
        None => format!("Revision {}", release.revision),
    };
    let body: AnyElement = if release.diff.is_empty() {
        let text = match previous {
            Some(previous) => format!(
                "Same chart and values as revision {}; Helm ran it again.",
                previous.revision
            ),
            None => "The first release; there is nothing earlier to compare with.".to_owned(),
        };
        div()
            .px(SPACE_8)
            .text_color(palette.fg2)
            .child(text)
            .into_any_element()
    } else {
        diff_view(release, previous)
    };
    div()
        .child(
            SectionHeader::new(title)
                .count(format!("{} · {}", release.chart_version, release.state)),
        )
        .child(body)
        .into_any_element()
}

fn diff_view(release: &HelmRevision, previous: Option<&HelmRevision>) -> AnyElement {
    let files = parse_unified(&release.diff);
    let old = previous.map_or_else(String::new, |p| format!("revision {}", p.revision));
    let new = format!("revision {}", release.revision);
    div()
        .px(SPACE_8)
        .child(DiffView::new("flux-release-diff", files).sides(old, new))
        .into_any_element()
}

fn glyph_column<R: 'static>(status: impl Fn(&R) -> Status + 'static) -> Column<R> {
    Column::new("", move |row: &R, _| {
        StatusGlyph::new(status(row)).into_any_element()
    })
    .fixed(GLYPH_COLUMN)
}

fn inventory_columns() -> Vec<Column<ManagedObject>> {
    vec![
        glyph_column(|o: &ManagedObject| o.status),
        Column::colored_text(
            "Kind",
            |o: &ManagedObject| o.kind.clone().into(),
            |_, p| p.mute,
        )
        .fixed(KIND_COLUMN),
        Column::text("Name", |o: &ManagedObject| o.path().into())
            .mono()
            .flex(NAME_WEIGHT, NAME_MIN),
        Column::new("Detail", |o: &ManagedObject, palette| {
            let color = if o.status == Status::Ok {
                palette.mute
            } else {
                palette.status(o.status)
            };
            div()
                .truncate()
                .font_family(FONT_MONO)
                .text_size(TEXT_SMALL)
                .text_color(color)
                .child(SharedString::from(o.detail.clone()))
                .into_any_element()
        })
        .flex(DETAIL_WEIGHT, DETAIL_MIN),
    ]
}

fn history_columns() -> Vec<Column<HistoryEntry>> {
    vec![
        Column::colored_text(
            "Age",
            |h: &HistoryEntry| h.age_label().into(),
            |_, p| p.mute,
        )
        .mono()
        .fixed(AGE_COLUMN),
        glyph_column(|h: &HistoryEntry| h.status),
        Column::colored_text(
            "Revision",
            |h: &HistoryEntry| h.revision.clone().into(),
            |_, p| p.acc,
        )
        .mono()
        .selectable()
        .auto(REVISION_COLUMN, REVISION_MAX),
        Column::colored_text(
            "Message",
            |h: &HistoryEntry| h.message.clone().into(),
            |_, p| p.fg2,
        )
        .mono()
        .tooltip(|h: &HistoryEntry| h.message.clone().into())
        .flex(1., Pixels::ZERO),
        Column::colored_text(
            "Count",
            |h: &HistoryEntry| h.count_label().unwrap_or_default().into(),
            |_, p| p.mute,
        )
        .mono()
        .end()
        .auto(COUNT_COLUMN, COUNT_MAX),
    ]
}

fn separator(after_text: bool) -> &'static str {
    if after_text { PHRASE_SEPARATOR } else { "" }
}

fn release_columns() -> Vec<Column<HelmRevision>> {
    vec![
        Column::colored_text(
            "Age",
            |r: &HelmRevision| time::age(r.age).into(),
            |_, p| p.mute,
        )
        .mono()
        .fixed(AGE_COLUMN),
        glyph_column(|r: &HelmRevision| r.status),
        Column::colored_text(
            "Chart",
            |r: &HelmRevision| r.chart_version.clone().into(),
            |_, p| p.acc,
        )
        .mono()
        .auto(REVISION_COLUMN, REVISION_MAX),
        Column::colored_text(
            "Description",
            |r: &HelmRevision| format!("{} · {}", r.state, r.description).into(),
            |_, p| p.fg2,
        )
        .mono()
        .flex(1., Pixels::ZERO),
        Column::colored_text(
            "Revision",
            |r: &HelmRevision| format!("#{}", r.revision).into(),
            |_, p| p.mute,
        )
        .mono()
        .end()
        .auto(COUNT_COLUMN, COUNT_MAX),
    ]
}

fn dependent_columns() -> Vec<Column<FluxObject>> {
    vec![
        glyph_column(FluxObject::status),
        Column::colored_text(
            "Kind",
            |o: &FluxObject| o.key.kind.label().into(),
            |_, p| p.mute,
        )
        .fixed(KIND_COLUMN),
        Column::text("Name", |o: &FluxObject| {
            format!("{}/{}", o.key.namespace, o.key.name).into()
        })
        .mono()
        .flex(1., NAME_MIN),
        Column::colored_text(
            "State",
            |o: &FluxObject| o.phase.note().into(),
            |o, p| {
                if o.status() == Status::Ok {
                    p.mute
                } else {
                    p.status(o.status())
                }
            },
        )
        .mono()
        .flex(1., DETAIL_MIN),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_deployers_fetch_before_reconciling() {
        assert!(fetch_label(FluxKind::Kustomization).is_some());
        assert!(fetch_label(FluxKind::HelmRelease).is_some());
        assert!(fetch_label(FluxKind::GitRepository).is_none());
    }

    #[test]
    fn spec_values_keep_phrases_together() {
        let chart = wrappable("kube-prometheus-stack · 62.7.0");
        assert_eq!(chart, "kube-prometheus-stack ·\u{a0}62.7.0");
        let timing = wrappable("30m · timeout 10m");
        assert_eq!(timing, "30m ·\u{a0}timeout\u{a0}10m");
    }
}
