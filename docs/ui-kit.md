# UI kit

`flock_ui::kit` holds the primitives every screen is built from. Use them instead of styling raw
`div`s; all colours, sizes and radii come from `flock_ui::theme` (`cx.palette()` and the `theme::*`
constants). If you need a value the theme lacks, add a named constant there.

See every primitive running: `cargo run -p flock -- --demo --screen kit` (`kit-overlays` opens
the menu, confirm dialog and a toast; `kit-panes` shows charts, split, code, diff, dock and logs),
`--appearance dark|light`. Screenshots:
`mise run shot -- kit dark`. The gallery source (`crates/flock-ui/src/gallery.rs`) is a working
usage example for everything below.

```rust
use flock_ui::kit::*;
use flock_ui::theme::{self, ActiveTheme};
```

Two kinds of primitive:

- **Elements** (`RenderOnce` builders): construct in `render` and pass to `.child(...)`. Stateless.
- **Views** (`Entity<T>`): create once with `cx.new(...)`, keep the entity in your view's struct,
  render it with `.child(entity.clone())`. They keep their own state (open, sorted, scrolled) and
  report back through `cx.subscribe` events.

Handlers have the shape `Fn(&Arg, &mut Window, &mut App)`, so `cx.listener(|this, arg, window, cx| ..)`
from your view fits directly.

## Status

| primitive | API |
|---|---|
| `StatusGlyph` | `StatusGlyph::new(Status::Err)`: the shape (● ▲ ✕ ◐ ‖ ○) in the status colour, 10px slot |
| `StatusText` | `StatusText::new(Status::Err, "CrashLoopBackOff")`: glyph + label, both in the status colour, truncates |

Row tints for a status: `palette.tint(status)` returns `okbg` / `warnbg` / `errbg` (or `None`).

## Text and layout

```rust
SectionHeader::new("Pods").count("3 · click to follow logs").end(Segmented::new("f")...)
SectionHeader::new("Problems").annotation(badges)  // any element right after the label
SectionLabel::new("Container")            // uppercase 11px mute label alone
Rule::horizontal()  /  Rule::vertical()   // 1px `line`
Link::new("rev", "a41e07c").mono().on_click(|_, window, cx| ..)
Highlighted::new("checkout-api", vec![0..8])  // byte ranges drawn in `acc` (fuzzy match hits)
Kbd::new("⌘K")
ClickableSection::new("overview-pods")    // whole-cell hover (`hover` token), → after the title on hover/focus, pointer
    .title(SectionLabel::new("Pods")).trailing(detail).padding(x, y)
    .child(body).on_activate(|window, cx| ..)  // click, Enter and Space
Prose::new("summary")                     // wrapping sentence with inline mono links
    .text("checkout-api ").link("a41e07c", |window, cx| ..).inert_link("no-target")
KeyValue::new()                           // inspector / spec grid, label column 72px muted
    .row("image", "ghcr.io/acme/checkout-api:2.14.0")
    .row("last exit", div().text_color(palette.err).child("1 (Error) · 2m ago"))
    .label_width(px)                      // optional; `.sans()` for proportional values
```

### Selectable text

Text people read and copy is selectable as in a browser: drag selects by character, double click a
word, triple click a line, ⇧-click extends, ⌘C copies, ⌘A selects the whole block last clicked.
The selection is window-wide (gpui-base's `TextSelection`, mounted by `Root`), so a drag can cross
lines and blocks; the highlight is the `sel` token and the cursor is an I-beam.

```rust
SelectableText::new(value)                      // plain text; id defaults to a hash of the text
    .id(("log-line", ix))                       // explicit id when equal texts sit side by side
    .highlights(ranges)                         // (Range<usize>, HighlightStyle), e.g. search hits
    .mono(range)                                // Plex Mono for a range
    .link(range, |window, cx| ..)               // clickable range; a drag over it still selects
KeyValue::new().text("zone", node.zone.clone()) // selectable value cell
Column::text("Object", ..).selectable()         // selectable table text cell
Selectable::new("code-text", content)           // several laid-out lines as one block:
    .run(line, styled.layout().clone())         // copy joins them with newlines, blank lines kept
    .whole_rows()                               // a drag may start anywhere on a row
```

`PageHeader` and `Inspector` titles, `Prose`, `CodeView`, `LogView` messages and `KeyValue::text`
values are selectable already. A drag that selects text in a table row does not select the row
(`kit::selectable::is_text_drag`); plain clicks still do. Truncated text is not selectable. The
terminal has its own cell-grid selection (drag, ⇧-click); ⌘C copies it, or the transcript when
nothing is selected.

### Raw messages

Text that comes verbatim from Kubernetes or another API (event, condition, error block, Problems and
Loco placement messages, error toast details, failed-load reasons) renders in `RawMessage`, a compact
inline code block: Plex Mono `TEXT_SMALL`, `raise` ground, 1px `line2` border, `RADIUS_SMALL`,
`SPACE_1` / `SPACE_3` padding. It wraps only at spaces (`SelectableWords`), so refs stay whole, and
copies the exact text. The border is always drawn, so hover and selection never change its size.
Labels, reasons and our own sentences stay plain text.

Table cells: plain text; RawMessage: inspectors, panels, headers.

```rust
RawMessage::new(event.message.clone())          // block: wraps at spaces, selectable
RawMessage::new(message).color(palette.err)     // text colour, default fg2
RawMessage::new(message).line()                 // one truncated line, for headers
```

`PageHeader` is the document header of a resource page (title, status, one primary action, the
⋯ menu, the one-sentence summary, a mono meta line):

```rust
PageHeader::new("checkout-api")
    .mono()
    .status(Status::Err, "2/3 ready")
    .action(Button::new("reconcile", "Reconcile").primary().shortcut("⌘R").on_click(..))
    .action(self.menu.clone())                 // Entity<ActionMenu>
    .sentence("One pod is crash-looping, so the rollout to revision 37 is stuck.")
    .meta(div().flex().gap(SPACE_4).child("payments ·").child(Link::new("ks", "apps-payments")))
    .footer(full_view_link("workload-full-view", &open))   // optional last line (compact panes)
```

`MetaLine` is a mono, muted line of facts under a list page's header (Cluster overview: distro,
version, region, metrics source; Nodes: the instance mix). List pages have no summary sentence
(decision 22); the first row under the header is the `FilterBar`.

```rust
MetaLine::new("14 nodes · 13 ready · 1 cordoned · m6i.xlarge ×12")
```

## Buttons and controls

```rust
Button::new("id", "Reconcile").primary().shortcut("⌘R")   // acc / onacc
Button::new("id", "Cancel")                               // secondary: line2 border (default)
Button::new("id", "Paste kubeconfig…").ghost()            // acc text, no border
Button::new("id", "Delete pod").danger()
Button::new("id", "follow").small().selected(self.follow) // dock toggles: sel/selfg/selline
    .disabled(bool) .tooltip("text") .on_click(|event, window, cx| ..)

IconButton::glyph("close", "×", "Close")                  // tooltip text is required
IconButton::svg("collapse", assets::CHEVRON_LEFT, "Collapse").bordered().selected(bool)

Segmented::new("severity").option("All").option("Errors").option("Warnings")
    .selected(ix).small().mono()
    .on_select(cx.listener(|this, ix: &usize, _, cx| ..))

Toggle::new("confirm", on).label("Confirm destructive actions")
    .on_toggle(cx.listener(|this, on: &bool, _, cx| ..))
Checkbox::new("ctx", checked).label("prod-us-east").on_toggle(..)
```

Tooltips on any stateful element: `.tooltip(Tooltip::text("Copy commit"))`.

### Text fields

Wraps gpui-component's input. Keep the state entity in your view:

```rust
let filter = cx.new(|cx| TextFieldState::new(window, cx).placeholder("filter  /regex/"));
cx.subscribe(&filter, |this, state, event: &InputEvent, cx| {
    if let InputEvent::Change = event { let q = state.read(cx).value(); .. }
}).detach();

TextField::new(&self.filter).prefix("path:").width(px)    // 24px, mono 12
TextField::new(&self.path).prefix("path:").compact()       // 22px, mono 11.5 (page header filters)
TextField::search(&self.filter)                            // 20px x 180px, mono 11.5 (dock filters)
TextField::bare(&self.query, theme::TEXT_PALETTE)          // no box, mono at the given size (palette)
```

The field's own key bindings win over a parent's. To steer a list from the field, catch its actions
in the capture phase on the parent: `.capture_action(cx.listener(|this, _: &TextFieldUp, _, cx| ..))`
(also `TextFieldDown`); `TextFieldEscape` and `InputEvent::PressEnter` bubble normally.

### Select

```rust
let select = cx.new(|cx| Select::new("context", vec!["prod-us-east".into(), ..], cx).mono().width(px));
cx.subscribe(&select, |this, _, SelectEvent::Changed(ix), cx| ..).detach();
select.read(cx).selected_index()
select.update(cx, |s, cx| s.set_selected(ix, cx));   // e.g. when the edited object changes
```

`set_details(details, cx)` adds muted secondary text per option, by index (empty for none), shown in the
list and after the selected label (Cluster settings shows the detected Prometheus target beside `auto`).
Settings forms keep controls within `theme::SETTINGS_FIELD` and inset by `theme::SETTINGS_INSET`.

`.editable(window, cx)` adds a type-to-filter field and a “Use” row for free text. Arrow keys move
through the filtered options and the free-text row; Enter saves, Esc cancels. `.placeholder("default")`
labels an empty value. `set_value(value, cx)` selects a value, retaining it as an option when absent
from the supplied list. Picks and free text both emit `SelectEvent::Changed`; read the value with
`selected_label()`.

Own popover (same look and keyboard as the action menu); arrow keys, Enter, Esc. Every kit popover
surface (`popover::surface`) occludes what is under it, so hover and clicks never reach rows beneath an
open menu, select or switcher. Options carry `{select}-option-{ix}` debug selectors. `.compact()` draws
the trigger as a plain 22px button without the chevron (page header filters such as `All clusters`).
`select.update(cx, |s, cx| { s.set_options(options, cx); s.set_selected(ix, cx) })` changes it from code
(no `Changed` event).

### FilterBar

The one filter row every list page uses, always the first row above the list and always in this
order: an optional scope picker (namespace, or cluster on Changes), the text filter, status chips
with counts, Clear, and `n of m` on the right while the list is narrowed. `/` focuses the current
page's bar (the shell routes it; ignored while typing in a field or terminal), Esc in the field
clears it. The text is parsed into `flock_core::filter::TextFilter`: plain words must all match,
`ns:<part>` matches the namespace, `l:key=value` / `label:key` match labels, as in the palette.

The field says “Filter by name, namespace or label”; its separate `/` key cap disappears while
entering a query. `ns:` completes cluster namespaces, `label:` / `l:` completes keys and then
values from the current list. ↑ / ↓ moves through suggestions, Tab or Enter completes, and Esc
closes suggestions before clearing the query. Completed tokens become removable chips; Backspace
with an empty text field removes the last chip. Plain words remain editable text. Enter keeps its
normal behavior when no suggestion is open.

Each list passes the labels of its rows with
`FilterBar::set_labels(flock_core::filter::label_values(rows.iter().map(|row| &row.labels)), cx)`;
the shell supplies the cluster's namespaces through `set_namespaces`. `FilterBar::text` returns the
chips and the typed text together.

```rust
let filter = cx.new(|cx| FilterBar::new("pod-filter", window, cx).scope(namespace_select.clone()));
cx.subscribe(&filter, |this, _, FilterEvent::Changed, cx| this.refresh(cx)).detach();
// in refresh: read the query and chip, filter rows in flock-core, then report back
let (query, chip) = { let bar = filter.read(cx); (bar.query().clone(), bar.chip()) };
filter.update(cx, |bar, cx| {
    bar.set_chips(counts.map(|(f, n)| chip_label(f.label(), n)).collect(), cx);
    bar.set_matches(shown, total, cx);
});
table.update(cx, |t, cx| t.set_highlight(query.words.clone(), cx));
```

Screens expose `filter_bar()` so the shell can focus it. On namespaced lists the scope picker
is the namespace: read it with `preferences.namespace_selection(list)`, and on a pick call
`preferences.select_namespace(list, selection)` and emit `Command::SetPreferences`. The rows hold
every namespace, so the picker only filters the view and the page's header note (decision 32).
`StatusFilter` (All / Needs attention)
covers lists whose only chip is "has a problem".

## ActionMenu (⋯ / Actions)

Each item has a label and an optional one-line description, shortcut hint, destructive styling and
disabled state. Opens below the trigger, right-aligned, fades in; ↑ ↓ Enter, Esc or a click outside
closes it and returns focus.

```rust
let menu = cx.new(|cx| {
    ActionMenu::new("ks-actions", cx)                 // ⋯ trigger
        .items(vec![
            MenuItem::new("Suspend", |window, cx| ..)
                .shortcut("⌘⇧S")
                .description("Stop reconciling until resumed. Drift stays in place."),
            MenuItem::new("Delete…", |window, cx| confirm(dialog(), window, cx))
                .destructive()
                .description("Prune every object this Kustomization manages."),
        ])
});
ActionMenu::new("workload-actions", cx).labeled("Actions", Some("⌘.".into())) // labelled trigger
menu.update(cx, |m, cx| m.toggle(window, cx));   // e.g. from a ⌘. key binding
menu.update(cx, |m, cx| m.set_items(items, cx));
```

`ActionMenu::new(id, cx).plain()` draws the ⋯ trigger without a border (for rows in the side panels).
`ActionMenu::context(id, cx)` has no trigger: `menu.show_at(position, items, window, cx)` opens it at a
window point (the Table uses it for row menus).

## Modal

A wider dialog with its own content, for flows such as Add cluster. Rendered by the view that owns
it (deferred above the window, below confirm dialogs and toasts); Esc and × call `on_close`.

```rust
Modal::new("add-cluster", self.flow.title(), theme::DIALOG_WIDE)
    .focus(&self.focus)                         // so Esc reaches it
    .on_close(move |window, cx| ..)
    .child(body)                                // flex column; give scrolling parts flex_1 + min_h_0
```

## ConfirmDialog and toasts

Both render in the window-wide overlay layer. Every root view must include it once:
`.child(kit::overlay_layer(cx))` (`Shell` and the gallery already do).

```rust
confirm(
    ConfirmDialog::new(
        "Delete pod",
        "The pod is deleted now. Its ReplicaSet creates a replacement within seconds; logs from this container are lost.",
        "Delete pod",                                  // danger button label
        |window, cx| { /* send the Command */ },
    )
    .detail("prod-us-east · payments/checkout-api-7f9c6d-x2lqp")    // mono lines, optional
    .field(state)                                  // optional TextFieldState, focused on open
    .primary(),                                    // acc confirm button instead of danger
    window,
    cx,
);

toast(Toast::new(Status::Ok, "Scaled checkout-api to 3").detail("prod-us-east · payments"), cx);
```

The dialog is 440px, 4px radius, over a scrim; Esc or Cancel dismisses, ⌘↵ confirms. Toasts stack
bottom-right above the status bar. Each has a leading status glyph in the severity colour, a
severity-tinted ground (`okbg` / `warnbg` / `errbg`, `accbg` for progress) with a flat 1px `line2` border and no shadow, and a ×; a click
dismisses it. Errors persist until dismissed. Success auto-dismisses after `theme::TOAST_SUCCESS_TIMEOUT`,
warnings after `TOAST_WARNING_TIMEOUT`, progress after `TOAST_PROGRESS_TIMEOUT`. An error toast's
detail is the raw failure and renders as a `RawMessage`; other details are plain `fg2` text.

```rust
toast(Toast::new(Status::Err, "Could not update Flock").detail(reason).action("Retry", |_, cx| ..), cx);
toast(Toast::new(Status::Progress, "Reconciling").key("reconcile"), cx);
toast(Toast::new(Status::Ok, "Reconciled").key("reconcile"), cx);   // replaces the progress toast in place
```

## Table

Generic over your row type, virtualised with `uniform_list`, sticky header, hover, selection
(`sel` fill), status-tinted rows, sortable headers (click cycles ↑ ↓ off), keyboard
↑ ↓ Enter.

```rust
let pods = cx.new(|cx| {
    Table::new("pods", vec![
        Column::text("Name", |p: &Pod| p.name.clone()).mono().flex(1.5, px(210.)).sort_by_key(|p: &Pod| p.name.clone()),
        Column::colored_text("Ready", |p: &Pod| p.ready.clone(), |p, palette| palette.status(p.status)).mono().fixed(px(44.)),
        Column::new("Status", |p: &Pod, _palette| StatusText::new(p.status, p.phase.clone()).into_any_element()).fixed(px(150.)),
        Column::text("Restarts", |p: &Pod| p.restarts.to_string().into()).mono().end().fixed(px(58.)),
    ], cx)
    .rows(pods)
    .row_status(|p: &Pod| (p.status != Status::Ok).then_some(p.status))  // okbg/warnbg/errbg tint
    .row_key(|p: &Pod| p.name.clone())   // keeps selection across updates, flashes rows whose status changed
    .empty("No pods match the filter.")
    .fit_content()                        // inline in a scrolling page; omit to fill the parent and scroll
});
cx.subscribe(&pods, |this, table, event: &TableEvent, cx| match event {
    TableEvent::Selected(ix) => ..,        // index into the rows you passed
    TableEvent::Opened(ix) => ..,          // Enter, double click, or a click with .click_opens()
}).detach();
pods.update(cx, |t, cx| t.set_rows(new_rows, cx));
```

Keyboard (in `kit::list_keys`, shared with the sidebar, switcher and palette): ↑ ↓, Home / End and
PageUp / PageDown move the selection, Enter emits `Opened`, typed letters jump to the first row whose
`.type_ahead(|row| name)` (or `row_key`, or the part of it after the last `/`) starts with them, and Esc
dispatches `actions::CloseInspector` (open row menus take Esc first). The table is a tab stop;
keyboard position is shown by the selected row fill, without a focus outline.

Column options: `.fixed(px)`, `.flex(weight, min)`, `.auto(min, max)` (width follows the widest cell text, so a
status or revision column never truncates while it has room; the column needs text to measure, from
`Column::text`, `colored_text`, `status_text` or `.fit_by(text, extra)` / `.fit_status_by(text)` on a custom cell; give
long free-text columns `.flex(..)` so they take the remainder and truncate first), `.end()`, `.center()`, `.mono()` (Plex Mono 12;
rows are otherwise Sans 12.5), `.sortable(cmp)` / `.sort_by_key(key)`, `.highlight()` (text columns
draw the filter's matches in `acc`, see `set_highlight`), `.fit_by(text, extra)` (what a custom cell
measures to when its border is double-clicked). `Column::status_text(title, status, text)` is a glyph
plus text cell.

Every header border is a resize handle (col-resize cursor, `{table}-resize-{ix}`): drag to resize down
to `theme::COLUMN_MIN` (or the flex minimum), double-click to fit the column to its widest text (custom
cells without `fit_by` go back to their design width). Widths are stored per scope and table id in
`flock_ui::prefs` (`view.json` in the Flock data dir, memory only in demo and tests); call
`table.set_scope(cluster.as_str(), cx)` on cluster pages, flock pages use `prefs::FLOCK_SCOPE`.

⌘-click and middle-click on a row emit `Opened` as well (the shell turns the resulting navigation
into a new tab). Rows that open something put `screens::new_tab_item(command, emit)` ("Open in new
tab") first in their menu.

`.multi_select()` lets ⇧-click add or remove rows from a marked set (drawn like the selection;
a plain click clears it). `marked_rows()` reads it, `clear_marks(cx)` empties it, and
`TableEvent::Marked` reports changes. It needs `row_key`.

Row actions: `.row_actions(|row| Vec<MenuItem>)` / `set_row_actions(..)` adds a ⋯ button on the hovered
row (`{table}-row-{ix}-more`) and a right-click menu (`{table}-row-menu-item-{ix}`), both with that
row's items. `hovered_row()` reads the hovered row. Table options:
`.row_height(px)` (default `ROW_TABLE` 26), `.headerless()`, `.sorted_by(col, dir)`,
`.column_gap(px)` (default `SPACE_6`), `.inset(px)` (leading row and header padding, default `SPACE_8`),
`.mono_header()` (Plex Mono 10.5 header titles, e.g. short cluster lanes),
`set_empty(text, cx)` changes the empty-table sentence when the reason changes, and
`.click_opens()` (a single click also emits `Opened`; use it on every list whose rows have no
inspector and lead to a separate page, so one click or Enter opens the page while the arrow keys
only move the selection) and
`.span_rows(|row, palette| Option<AnyElement>)`: rows for which it returns `Some` render that
element across the full width instead of cells (group headers). `set_columns(columns, cx)` swaps
columns at runtime (e.g. one per cluster).
Column widths are design values; define them as `const`s at the top of your screen file.
When the box is narrower than the columns' minimum widths (fixed widths, flex minimums, gaps and
padding), the table scrolls horizontally inside its box; header and rows move together.
`table.read(cx).min_width()` is that minimum; `min_row_width(widths, gap, padding)` gives the same
for hand-built rows.

## Matrix

Objects × clusters grid (GitOps sync state, compare views): a header row of column titles, uppercase
group rows on `panel`, and 38px (`ROW_MATRIX`) rows of a label plus one cell per column. Columns
share the width evenly above their minimum; each cell has a left rule, an optional tint and, when
clickable, a pointer plus a `selline` outline on hover.

```rust
Matrix::new("sync", "Object", vec!["dev-kind".into(), "staging".into()])
    .min_widths(px(220.), px(150.))
    .group("Kustomizations", "6 of 12")
    .row(label, vec![
        MatrixCell::new(content).tint(Some(palette.errbg)).on_click(cx.listener(|_, _, _, cx| ..)),
        MatrixCell::new(not_deployed),
    ])
```

## Split panes

```rust
Split::horizontal("workload-split", main_content, inspector)   // or Split::vertical
    .ratio(0.6)                       // initial share of the first pane
    .collapsed(self.inspector_hidden) // second pane animates to zero and back
```

Element; the divider drags (each pane keeps at least `SPLIT_MIN`) and the ratio is remembered per
id for the session, so recreating the screen restores it. Children should fill their pane
(`size_full`, own scrolling).

`.line(palette.line2)` recolours the divider and `.on_resize(|share, window, cx| ..)` hands the
ratio to the caller instead of the session store; the caller then passes it back through
`.ratio(..)` on every render. The shell's two panes use both (`Split::horizontal("panes", ..)`,
divider selector `panes-divider`) and keep the share in `tabs::Divider`, saved with the window.

## Panes and compact views

With two panes (`tabs::MAX_PANES`) each pane is a column of its own tab strip (ending in the 32px
Split right button, `split-right` / `pane-1-split-right`) over its page; selectors for the second
pane carry a `pane-1-` prefix (`pane-1-tab-0`). A split pane renders compact views:
`WorkloadScreen::set_compact(Some(full_view))` draws the header (name, status, cluster), the
sentence, `Workload::fields()` as 26px label/value rows and a three-column Pods table, without the
action menu, inspector or dock; `FluxObjectPage::set_compact` draws only the header block;
`LogsScreen::set_compact(true)` draws the log lines alone. `screens::full_view_link(id, &open)`
draws the "Open full view →" link (`PageHeader::footer`), which joins the panes into one with that
tab active. Other pages render their full page.

## Dock (Logs / Events / Terminal)

Matches `Workload.dc.html`: a 30px `chrome` bar of tabs (label + mono detail), the active tab
joins the content, the active tab's tools sit on the right next to a collapse chevron. Clicking
the active tab or the chevron collapses the dock to its bar with an animation.

```rust
let dock = cx.new(|_| DockState::new());          // keep in your view
Dock::new(&self.dock)
    .tab(DockTab::new("Logs").detail("x2lqp · api").tools(log_tools).content(self.logs.clone()))
    .tab(DockTab::new("Events").detail("7").content(self.events.clone()))
    .tab(DockTab::new("Terminal").detail("debug").content(terminal))
    .height(px)                                    // default DOCK_HEIGHT 250
dock.update(cx, |d, cx| d.select(0, cx));          // e.g. clicking a pod opens its logs
```

Place it as the last child of a flex column; it is `flex_none`.

## LogView

Virtualised (`uniform_list`) view over a ring buffer of `LOG_CAPACITY` lines; oldest lines drop.
Columns: timestamp (toggle), level (coloured; ERROR/FATAL rows get `errbg`), message.

```rust
let logs = cx.new(|_| LogView::new("pod-logs"));
logs.update(cx, |l, cx| l.push(LogLine::parse(raw_line), cx));   // strips ANSI, splits RFC3339 timestamp + level
logs.update(cx, |l, cx| l.extend(lines, cx));
LogLine::new(Some("21:12:20.231"), Some(Level::Error), "open /etc/checkout/stripe.env: …")
l.set_follow(bool, cx)       // follow tail; scrolling up turns it off
l.set_query(text, cx)        // case-insensitive highlight, jumps to first match
l.set_timestamps(bool, cx)   // l.timestamps() reads it
l.export()                   // plain text of the buffer, one line per entry (for downloads)
l.set_wrap(bool, cx)         // wrap long lines (switches to a variable-height list)
l.clear(cx)
```

`strip_ansi(&str)` is public for other text that comes from a terminal.

Lines can carry a source tag, drawn as a `LOG_SOURCE`-wide column in that source's colour
(`highlight::source_colors(palette)`: eight hues whose lightness starts at `acc` and steps until
the text is ≥ 4.5:1 on `editor`, ERROR rows and `sel`, so every theme and highlight stays legible):

```rust
LogLine::new(Some("21:14:00.211"), Some(Level::Info), "GET /healthz").with_source("x2lqp", slot)
l.set_lines(lines, cx)       // replace the buffer (a re-filtered or re-merged view)
l.set_highlight(text, cx)    // highlight matches without jumping or leaving follow
```

### Merged pod logs

`screens::multilog::MultiLogs` follows every pod and container of a `flock_core::LogTarget`
(a workload or Job and its pods, a label selector in a namespace, or a fixed set of pods) through
`Command::StreamPodLogs`. Lines merge in timestamp order into a `flock_core::multilog::MergedLog`
(bounded by `MERGED_LINES`); each pod keeps a colour slot from `ColorSlots` while it is present.
The legend chips (`{id}-source-{n}`) toggle a source; the filter field hides lines that do not
match and highlights the rest; `pause` holds new lines (counting them) until resumed; `follow`,
`timestamps` and `wrap` drive the `LogView`. Pods that start join and deleted pods leave
(`left`); a container whose stream ends shows `ended`. `MultiLogs::tools(cx)` returns the toolbar
for a dock bar or page row. `MultiLogsScreen` is the `Logs <target>` tab (`Location::logs`); the
workload dock's `all pods` toggle shows the same view for the workload.

## Terminal

A VT100/xterm terminal over a byte stream (exec / debug sessions), parsed by the `vt100` crate:
mono 12, `LOG_LINE` rows, a grid sized to the pane (`TerminalEvent::Resize` on every change),
10 000 rows of scrollback, ANSI 16/256/truecolour mapped onto the palette, bold, dim, italic,
underline, inverse, the alternate screen and an inverted block cursor while open. Cursor-position
and device-attribute queries are answered. Keys are emitted as tty bytes (arrows follow the
application-cursor mode, ⌃ letters, ⌥ as meta, Tab and ⇧Tab bound in the `Terminal` key
context so focus navigation does not take them); ⌘V pastes (bracketed when asked), ⌘C copies
the transcript. Notes (the equivalent `kubectl` line, session ended) are muted lines in the
transcript. The view does not echo. Sessions start only from an explicit "Open shell" button.

```rust
let terminal = cx.new(|cx| Terminal::new("shell", cx));
cx.subscribe(&terminal, |this, _, event: &TerminalEvent, cx| ..).detach();   // Input(bytes) / Resize(size)
terminal.update(cx, |t, cx| { t.clear(cx); t.note("$ kubectl exec …", cx); t.set_open(true, cx); });
terminal.update(cx, |t, cx| t.feed(&bytes, cx));
```

## CodeView and DiffView

```rust
CodeView::new("spec", yaml_text).yaml()     // line numbers, horizontal scroll, Copy button (toast on copy)
    .without_copy()
```

Every code view has Soft wrap and Expand controls. Scrolling stays inside the view in both axes;
line numbers stay fixed when scrolling horizontally and follow their lines vertically. Expand
opens YAML across the window; Esc or × returns focus to the page.

YAML highlighting is deliberately minimal: keys `acc`, comments and punctuation `mute`, values `fg`.

```rust
let files: Vec<DiffFile> = parse_unified(&git_diff_output);   // classifies +/-/context/@@/meta, numbers lines
DiffView::new("diff", files).sides("live", "a41e07c")

DiffFile::new("Deployment/payments/checkout-api · rendered for prod-us-east", vec![
    DiffLine::new(LineKind::Context, Some(41), "      - name: api"),
    DiffLine::new(LineKind::Added, Some(48), "              key: webhook_secret")
        .note("set by overlays/prod-us-east/patches/stripe.yaml:9 · unchanged 214d"),  // warnbg + ← note
])
```

Diffs are always rendered in full (no folding). Gutters: line number (new side for context and
additions, old side for removals) and a `+` / `−` sign in `addfg` / `delfg` over `addbg` / `delbg`.

## CodeEditor

gpui-component's code editor with the kit's YAML colours (keys `acc`, comments and punctuation
`mute`), line numbers and inline error marks. Keep the state entity in your view:

```rust
let editor = cx.new(|cx| yaml_editor(window, cx));          // CodeEditorState; InputEvent::Change on edits
editor.update(cx, |e, cx| highlight_yaml(e, *cx.palette(), cx));  // again when the palette changes
editor.update(cx, |e, cx| mark_error(e, Some(line), Some("…"), cx)); // zero-based line; None clears
CodeEditor::new(&editor)                                    // fills its parent, mono 12
```

The Edit YAML tab (`screens::edit::EditScreen`) is the working example.

## Meters and charts

```rust
Meter::new(0.61).request(0.74).limit(1.0)           // 6px bar, request marker fg2, limit marker warn; .color(c)
MiniMeter::new(0.42).color(palette.acc)             // 36×4 inline bar for table cells; fg2 by default
StackedBar::new(vec![(Status::Ok, 398.), (Status::Warn, 9.), (Status::Err, 5.)])   // .thin() for 4px
LineChart::new(100.)                                 // y max
    .series(Series::new("CPU", palette.acc, cpu).area(palette.accbg))
    .series(Series::new("Memory", palette.ok, mem))
    .threshold(74., "CPU requests", palette.warn)    // dashed line + legend entry
    .y_labels(labels_top_to_bottom)                  // also sets the number of gridlines
    .x_labels(labels_left_to_right)
    .height(px)                                      // default CHART_HEIGHT 170
    .title(SectionLabel::new("Utilization"))         // left of the legend, same row
    .message(EmptyState::new(..).centered())          // replaces the grid and both axes
```

The first x label aligns left, the last right and the rest centre on their ticks, so none is clipped.

Values outside 0..=1 (or 0..=max) are clamped; NaN reads as 0.

## Inspector

The side panel next to a list (`panel` background, rule on the left, scrolls on its own). A
`PanelHeader`, error blocks when something is wrong, then collapsible sections.

```rust
Inspector::new("node-inspector", "ip-10-4-31-12")
    .mono()
    .status(Status::Warn, "SchedulingDisabled")
    .actions(div().flex().gap(SPACE_3).child(primary).child(close))
    .issues(node.issue())                  // ErrorBlocks; nothing when empty
    .section(None, KeyValue::new().row("zone", "us-east-1c"))   // plain block
    .section(Some("Conditions"), conditions)                    // PanelSection
    .panel(PanelSection::events("node-inspector", events))
    .width(px(480.))                       // default `theme::INSPECTOR` (360)
```

### Panel sections, error blocks, labels and events

Every side panel, including custom layouts, uses the same pieces from `kit/panel.rs`:

```rust
PanelSection::new("pod-inspector", "Containers")   // id pod-inspector-containers
    .count("1 · 1 restart").count_color(palette.warn)
    .trailing(Link::new("all", "All →"))           // right of the header, does not toggle
    .closed()                                      // default when nothing is saved
    .child(body)
PanelSection::events(panel, events)                // "N · last 1h", EventList body
PanelSection::labels(panel, labels)                // count + LabelChips
PanelHeader::new("pod-inspector", name)          // one row, theme::PANEL_HEADER high
    .mono()
    .status(pod.status, pod.reason.clone())        // glyph + word beside the name, never wraps
    .action(shell)                                 // right-aligned
ErrorBlocks::new(issues).on_open(|ix, window, cx| ..)  // top of a panel
issue_stack(issues)                                // the same blocks inside a header (split panes)
EventRow::new(event).on_click("namespace-event-0", ..)
```

`PanelHeader` truncates the name first (full name in a tooltip) and keeps the same height for every
object and status; it carries `{panel}-header`, `-header-title` and `-header-status`. `PageHeader`
follows the same rule on its title row. A section header toggles on click (`{id}-toggle`, body `{id}-body`) and the state is saved in
`AppSettings::open_sections` under the section id. Error blocks carry `error-block-{ix}`; an empty
event list renders `events-empty` ("No events in the last 1h").

Drag the left edge to resize between `theme::INSPECTOR_MIN` and `INSPECTOR_MAX`; double-click
resets to the builder width. Width is saved per inspector id in the existing app settings (`settings.json` in the Flock data
dir). Custom panel layouts use `InspectorPane::new("kind", content).width(initial_width)` to get
the same behavior.

Inspectors that come and go animate with `Presence` (keep it in the screen) and the slide helpers:

```rust
let current = self.selected_node().cloned();
self.presence.track(current.as_ref(), presence, cx);  // fn presence(s: &mut NodesScreen) -> &mut Presence<Node>
let inspector = match current {
    Some(node) => Some(slide_in(self.inspector(&node, cx), "node-inspector", &node.name, INSPECTOR)),
    None => self.presence.leaving().cloned().map(|node| slide_out(self.inspector(&node, cx), "node-inspector", INSPECTOR)),
};
swap_in(content, "pod-inspector", &selection)   // always-present panes: fade and slide on selection change
```

`slide_in` grows the width and fades in once, then replays a short fade and slide whenever the
selection key changes; `slide_out` keeps the last selection on screen for `INSPECTOR_OUT`. Reduced
motion drops the exit at once.

## Relative time

`flock_core::time::age(duration) -> "4s" | "2m" | "3h" | "9d" | "1y"` and
`age_since(then, now)` (future timestamps read as `0s`).

Text columns may use `.tooltip(|row| row.message.clone().into())` to expose a full value on hover
while keeping truncation, filtering highlights and automatic column sizing.

## Loading, empty and error states

Every cluster page ends in one of loaded, empty or error. The backend reports
`Snapshot::pages[cluster][page]` as `WatchState::Pending(step)` (never loaded), `Synced`,
`Refreshing(reason)` (loaded once, now retrying; the page keeps its data) or `Failed(reason)`; a
completed initial list with no objects is `Synced`. `AppState` keeps the last detail and pod rows of
a connected cluster while a snapshot carries none for it, so a page with data in memory never shows
a skeleton again (decision 32). The shell renders:

- `Skeleton::rows()` for tables and `Skeleton::blocks()` for meters and charts, shimmering over
  `theme::SKELETON_SHIMMER` (still under reduced motion)
- `ConnectionProgress::new(cluster, step)` above the skeleton while the cluster is connecting
- `EmptyState::new(sentence).detail(text).action(button)`: no icon, one sentence, an optional muted line and buttons.
  `.centered()` centres it, for a chart's plot area.
  `EmptyState::for_list(&Emptiness, Noun, cluster, &filter, show_all)` builds the list variants below
- `ErrorState::new(note, retry)`: status glyph, our own note on one line, Retry;
  `ErrorState::raw(reason, retry)` shows a backend or API reason in a `RawMessage` instead

The filter bar is always the first row of a list page, so every state sits under it: the shell draws
the page's `FilterBar` above the skeleton, connection progress and error, and each list screen draws
its own empty state in place of the table. `flock_core::emptiness::Emptiness::classify` tells three
empties apart: the cluster has none (`No Deployments in <cluster>`, no button), the namespace picker
hides them (`No Deployments in namespace <ns>`, `6 Deployments in other namespaces`, primary
`Show all namespaces`), or the text or chip filter hides them (`No Deployments match "<text>"`, primary
`Clear filter`). Only Retry on error states is a button that reloads; empty states have no Refresh.

A page still pending after `flock_core::watch::WATCH_SYNC_TIMEOUT` becomes an error naming the step
it stalled on. Render the states headlessly with `mise run shot -- resources dark --state loading`
(also `connecting`, `empty`, `error`); `Ui::with_state` drives them in interaction tests.

## Pane history controls

`HeaderNavigation` renders ghost SVG Back and Forward `IconButton`s before the page title. The
shell supplies the pane's active `Tab` and snapshot; destinations use `Location::history_label`.
An empty history direction disables its button without changing geometry. Enabled tooltips name
the destination and ⌘[ or ⌘]. Right-click opens the shared `ActionMenu` in nearest-first order;
choosing a row emits `HistoryJump` with the owning tab, direction and step count. The shell focuses
that pane and restores its location through the existing navigation replay. Changing the active
tab or history closes an open menu. `PageHeader::leading` and `PageTitle::leading` host the controls
in screen-owned headers; shell-owned headers mount the same entity directly.
