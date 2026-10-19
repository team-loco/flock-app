use flock_core::app_settings::{
    CheckInterval, Highlight, PollInterval, Rgb, SnapshotCap, ThemeChoice,
};
use flock_core::{AppSettings, Command, Page, Status};
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, div, relative,
};

use crate::highlight;
use crate::keymap;
use crate::kit::{
    Button, InputEvent, Kbd, Link, SectionLabel, Segmented, TextField, TextFieldState, Toast,
    Toggle, Tooltip, toast,
};
use crate::settings::{self, Settings};
use crate::theme::{
    ActiveTheme, Appearance, HEX_FIELD, LINE_PROSE, PAGE_HEADER, Palette, RADIUS, RADIUS_SMALL,
    ROW_DOCUMENT, RULE, SETTINGS_COLUMN, SETTINGS_INSET, SPACE_1, SPACE_2, SPACE_4, SPACE_6,
    SPACE_7, SPACE_8, SWATCH, TEXT_SMALL, TEXT_TITLE, Theme,
};
use crate::updater::{self, Updater};

const APPEARANCE: &str = "Appearance";
const THEME: &str = "Theme";
const THEME_HINT: &str = "System follows macOS as it switches between light and dark.";
const HIGHLIGHT_SENTENCE: &str = "Pick a preset or type a hex colour. Flock derives the light and \
dark variants and adjusts their lightness so text stays readable.";
const PRIMARY: &str = "Primary highlight";
const PRIMARY_HINT: &str = "Links, primary buttons, the focus ring and the active tab.";
const SECONDARY: &str = "Secondary highlight";
const SECONDARY_HINT: &str = "Selected rows and navigation.";
const HEX_PLACEHOLDER: &str = "#hex";
const BAD_HEX: &str = "Expected a colour like #3a7bd5.";
const REFRESH: &str = "Data refresh";
const REFRESH_SENTENCE: &str =
    "Resource changes are pushed by Kubernetes watches as they happen; only metrics are polled.";
const METRICS_SERVER: &str = "metrics-server";
const METRICS_SERVER_HINT: &str = "Node and pod usage on every connected cluster.";
const PROMETHEUS: &str = "Prometheus";
const PROMETHEUS_HINT: &str = "Utilization history on cluster overviews.";
const SNAPSHOTS: &str = "Screen updates";
const SNAPSHOTS_HINT: &str = "Flock redraws at most this often, however busy the clusters are.";
const NOTIFICATIONS: &str = "Notifications";
const NEW_PROBLEMS: &str = "New problems";
const NEW_PROBLEMS_HINT: &str =
    "A macOS notification when a resource starts failing. A flapping pod is announced once.";
const WARNINGS: &str = "Include warnings";
const WARNINGS_HINT: &str = "Also notify for warnings, not only errors.";
const MUTE_PREFIX: &str = "Mute a cluster from its settings in ";
const MUTE_SUFFIX: &str = ".";
const UNBUNDLED: &str =
    "Flock is running outside Flock.app, so macOS will not show notifications from this copy.";
const SAVE_FAILED: &str = "Settings could not be saved";
const SHORTCUTS: &str = "Keyboard shortcuts";
const UPDATES: &str = "Updates";
const AUTOMATIC: &str = "Check automatically";
const AUTOMATIC_HINT: &str = "Look for a new release on GitHub at launch and then on an interval.";
const INTERVAL: &str = "Check every";
const INTERVAL_HINT: &str = "How often Flock looks again while it runs.";
const PRERELEASES: &str = "Include pre-releases";
const PRERELEASES_HINT: &str = "Also offer beta and release-candidate builds.";
const CHECK_NOW: &str = "Check now";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Primary,
    Secondary,
}

impl Slot {
    const ALL: [Self; 2] = [Self::Primary, Self::Secondary];

    const fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }

    fn get(self, settings: &AppSettings) -> Highlight {
        match self {
            Self::Primary => settings.primary,
            Self::Secondary => settings.secondary,
        }
    }

    fn set(self, settings: &mut AppSettings, highlight: Highlight) {
        match self {
            Self::Primary => settings.primary = highlight,
            Self::Secondary => settings.secondary = highlight,
        }
    }

    fn swatch(self, highlight: Highlight, appearance: Appearance) -> Hsla {
        let base = Palette::for_appearance(appearance);
        let derived = highlight::apply(base, appearance, highlight, highlight);
        match self {
            Self::Primary => derived.acc,
            Self::Secondary => derived.selline,
        }
    }
}

pub struct AppSettingsScreen {
    pub(crate) navigation: Option<Entity<crate::kit::HeaderNavigation>>,
    hex: [Entity<TextFieldState>; 2],
    invalid: [bool; 2],
    _settings: Subscription,
    _hex_events: [Subscription; 2],
    _updater: Subscription,
}

impl EventEmitter<Command> for AppSettingsScreen {}

impl AppSettingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe_global::<Settings>(|_, cx| cx.notify());
        let updater = cx.observe_global::<Updater>(|_, cx| cx.notify());
        let current = settings::current(cx).clone();
        let hex = Slot::ALL.map(|slot| {
            let value = match slot.get(&current) {
                Highlight::Custom(rgb) => rgb.to_string(),
                _ => String::new(),
            };
            cx.new(|cx| {
                TextFieldState::new(window, cx)
                    .placeholder(HEX_PLACEHOLDER)
                    .default_value(value)
            })
        });
        let hex_events = Slot::ALL.map(|slot| {
            let field = hex[slot.index()].clone();
            cx.subscribe(&field, move |this, field, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = field.read(cx).value();
                    this.typed(slot, &text, cx);
                }
            })
        });
        Self {
            navigation: None,
            hex,
            invalid: [false; 2],
            _settings: observe,
            _hex_events: hex_events,
            _updater: updater,
        }
    }

    fn typed(&mut self, slot: Slot, text: &str, cx: &mut Context<Self>) {
        let parsed = (!text.trim().is_empty()).then(|| text.parse::<Rgb>());
        let invalid = matches!(parsed, Some(Err(_)));
        if let Some(Ok(rgb)) = parsed {
            change(cx, |s| slot.set(s, Highlight::Custom(rgb)));
        }
        if let Some(flag) = self.invalid.get_mut(slot.index()) {
            *flag = invalid;
        }
        cx.notify();
    }

    fn pick(&mut self, slot: Slot, preset: Highlight, window: &mut Window, cx: &mut Context<Self>) {
        change(cx, |s| slot.set(s, preset));
        if let Some(field) = self.hex.get(slot.index()) {
            field.update(cx, |field, cx| field.set_value(String::new(), window, cx));
        }
        if let Some(flag) = self.invalid.get_mut(slot.index()) {
            *flag = false;
        }
        cx.notify();
    }

    fn highlight_row(
        &self,
        slot: Slot,
        current: &AppSettings,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let appearance = cx.global::<Theme>().appearance;
        let chosen = slot.get(current);
        let swatches: Vec<AnyElement> = Highlight::PRESETS
            .iter()
            .map(|preset| {
                let preset = *preset;
                let fill = slot.swatch(preset, appearance);
                let selected = chosen == preset;
                let label = preset.label();
                let selector = format!("{}-{}", slot.id(), label.to_lowercase());
                div()
                    .id(ElementId::Name(selector.clone().into()))
                    .debug_selector(move || selector.clone())
                    .size(SWATCH)
                    .flex_none()
                    .p(SPACE_1)
                    .rounded(RADIUS)
                    .border(RULE)
                    .border_color(if selected {
                        palette.fg
                    } else {
                        gpui::transparent_black()
                    })
                    .cursor_pointer()
                    .tooltip(Tooltip::text(label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick(slot, preset, window, cx);
                    }))
                    .child(div().size_full().rounded(RADIUS_SMALL).bg(fill))
                    .into_any_element()
            })
            .collect();
        let field = self.hex.get(slot.index()).map(|field| {
            div()
                .debug_selector(move || format!("{}-hex", slot.id()))
                .child(TextField::new(field).width(HEX_FIELD).compact())
        });
        let control = div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .children(swatches)
            .child(div().w(SPACE_4))
            .children(field);
        let invalid = self.invalid.get(slot.index()).copied().unwrap_or_default();
        let (title, hint) = match slot {
            Slot::Primary => (PRIMARY, PRIMARY_HINT),
            Slot::Secondary => (SECONDARY, SECONDARY_HINT),
        };
        let error = invalid.then(|| {
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.status(Status::Warn))
                .child(BAD_HEX)
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(row(title, hint, control, palette))
            .children(error)
            .into_any_element()
    }

    fn header(&self, palette: &Palette) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(PAGE_HEADER)
            .px(SPACE_8)
            .border_b(RULE)
            .border_color(palette.line)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .children(self.navigation.clone())
            .child(
                div()
                    .text_size(TEXT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Page::Settings.title()),
            )
    }

    fn appearance(current: &AppSettings, palette: &Palette) -> gpui::Div {
        let selected = ThemeChoice::ALL
            .iter()
            .position(|choice| *choice == current.theme)
            .unwrap_or_default();
        let choices = ThemeChoice::ALL
            .iter()
            .fold(Segmented::new("settings-theme"), |control, choice| {
                control.option(choice.label())
            })
            .selected(selected)
            .on_select(|ix, _, cx| {
                if let Some(choice) = ThemeChoice::ALL.get(*ix).copied() {
                    change(cx, |s| s.theme = choice);
                }
            });
        section(APPEARANCE, palette).child(row(THEME, THEME_HINT, choices, palette))
    }

    fn highlights(
        &self,
        current: &AppSettings,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![prose(HIGHLIGHT_SENTENCE, palette).into_any_element()];
        for slot in Slot::ALL {
            rows.push(self.highlight_row(slot, current, palette, cx));
        }
        rows
    }

    fn refresh(
        current: &AppSettings,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let refresh = current.refresh;
        let metrics = intervals("settings-metrics", refresh.metrics).on_select(cx.listener(
            |_, ix: &usize, _, cx| {
                if let Some(interval) = PollInterval::ALL.get(*ix).copied() {
                    Self::set_refresh(cx, |s| s.refresh.metrics = interval);
                }
            },
        ));
        let prometheus = intervals("settings-prometheus", refresh.prometheus).on_select(
            cx.listener(|_, ix: &usize, _, cx| {
                if let Some(interval) = PollInterval::ALL.get(*ix).copied() {
                    Self::set_refresh(cx, |s| s.refresh.prometheus = interval);
                }
            }),
        );
        let selected = SnapshotCap::ALL
            .iter()
            .position(|cap| *cap == refresh.snapshots)
            .unwrap_or_default();
        let snapshots = SnapshotCap::ALL
            .iter()
            .fold(Segmented::new("settings-snapshots"), |control, cap| {
                control.option(cap.label())
            })
            .mono()
            .selected(selected)
            .on_select(cx.listener(|_, ix: &usize, _, cx| {
                if let Some(cap) = SnapshotCap::ALL.get(*ix).copied() {
                    Self::set_refresh(cx, |s| s.refresh.snapshots = cap);
                }
            }));
        section(REFRESH, palette)
            .child(prose(REFRESH_SENTENCE, palette))
            .child(row(METRICS_SERVER, METRICS_SERVER_HINT, metrics, palette))
            .child(row(PROMETHEUS, PROMETHEUS_HINT, prometheus, palette))
            .child(row(SNAPSHOTS, SNAPSHOTS_HINT, snapshots, palette))
    }

    fn notifications(
        current: &AppSettings,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let notifications = current.notifications;
        let enabled =
            Toggle::new("settings-notify", notifications.enabled).on_toggle(|on, _, cx| {
                let on = *on;
                change(cx, |s| s.notifications.enabled = on);
            });
        let warnings =
            Toggle::new("settings-warnings", notifications.warnings).on_toggle(|on, _, cx| {
                let on = *on;
                change(cx, |s| s.notifications.warnings = on);
            });
        let clusters = Link::new("settings-clusters", Page::ClusterSettings.label()).on_click(
            cx.listener(|_, _, _, cx| cx.emit(Command::SelectPage(Page::ClusterSettings))),
        );
        let mute = div()
            .flex()
            .text_size(TEXT_SMALL)
            .text_color(palette.mute)
            .child(MUTE_PREFIX)
            .child(clusters)
            .child(MUTE_SUFFIX);
        let unbundled = (!settings::bundled(cx)).then(|| {
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.status(Status::Warn))
                .child(UNBUNDLED)
        });
        section(NOTIFICATIONS, palette)
            .child(row(NEW_PROBLEMS, NEW_PROBLEMS_HINT, enabled, palette))
            .child(row(WARNINGS, WARNINGS_HINT, warnings, palette))
            .child(mute)
            .children(unbundled)
    }

    fn set_refresh(cx: &mut Context<Self>, edit: impl FnOnce(&mut AppSettings)) {
        let before = settings::current(cx).refresh;
        change(cx, edit);
        let after = settings::current(cx).refresh;
        if after != before {
            cx.emit(Command::SetRefresh(after));
        }
    }
}

fn updates(current: &AppSettings, palette: &Palette, cx: &App) -> impl IntoElement {
    let updates = current.updates;
    let automatic =
        Toggle::new("settings-update-automatic", updates.automatic).on_toggle(|on, _, cx| {
            let on = *on;
            change(cx, |s| s.updates.automatic = on);
        });
    let selected = CheckInterval::ALL
        .iter()
        .position(|interval| *interval == updates.interval)
        .unwrap_or_default();
    let interval = CheckInterval::ALL
        .iter()
        .fold(
            Segmented::new("settings-update-interval"),
            |control, interval| control.option(interval.label()),
        )
        .mono()
        .selected(selected)
        .on_select(|ix, _, cx| {
            if let Some(interval) = CheckInterval::ALL.get(*ix).copied() {
                change(cx, |s| s.updates.interval = interval);
            }
        });
    let prereleases =
        Toggle::new("settings-update-prereleases", updates.prereleases).on_toggle(|on, _, cx| {
            let on = *on;
            change(cx, |s| s.updates.prereleases = on);
        });
    let version = format!("Flock {}", updater::version());
    let state = updater::summary(&updater::status(cx).state);
    let checked = updater::last_checked(cx);
    let detail = state.map_or_else(|| checked.clone(), |state| format!("{state} · {checked}"));
    let check = Button::new("settings-check-now", CHECK_NOW)
        .small()
        .on_click(|_, _, cx| updater::check_now(cx));
    let about = div()
        .flex()
        .items_center()
        .gap(SPACE_8)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(SPACE_1)
                .child(div().text_color(palette.fg).child(version))
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(detail),
                ),
        )
        .child(div().flex().flex_none().child(check));
    section(UPDATES, palette)
        .child(row(AUTOMATIC, AUTOMATIC_HINT, automatic, palette))
        .child(row(INTERVAL, INTERVAL_HINT, interval, palette))
        .child(row(PRERELEASES, PRERELEASES_HINT, prereleases, palette))
        .child(about)
}

fn shortcuts(palette: &Palette) -> impl IntoElement {
    let groups = keymap::grouped().into_iter().map(|(group, bindings)| {
        let rows = bindings.into_iter().map(|binding| {
            let keys = binding
                .keys
                .iter()
                .map(|keys| Kbd::new(keymap::display(keys)));
            div()
                .flex()
                .items_center()
                .gap(SPACE_8)
                .h(ROW_DOCUMENT)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(palette.fg2)
                        .child(binding.description),
                )
                .child(div().flex().flex_none().gap(SPACE_2).children(keys))
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .pb(SPACE_2)
                    .text_size(TEXT_SMALL)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.fg)
                    .child(group.label()),
            )
            .children(rows)
    });
    section(SHORTCUTS, palette).children(groups)
}

fn change(cx: &mut App, edit: impl FnOnce(&mut AppSettings)) {
    if let Err(error) = settings::update(cx, edit) {
        tracing::warn!(%error, "saving settings");
        toast(Toast::new(Status::Err, SAVE_FAILED).detail(error), cx);
    }
}

fn intervals(id: &'static str, current: PollInterval) -> Segmented {
    let selected = PollInterval::ALL
        .iter()
        .position(|interval| *interval == current)
        .unwrap_or_default();
    PollInterval::ALL
        .iter()
        .fold(Segmented::new(id), |control, interval| {
            control.option(interval.label())
        })
        .mono()
        .selected(selected)
}

fn section(label: &'static str, palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(SPACE_6)
        .py(SPACE_7)
        .border_b(RULE)
        .border_color(palette.line)
        .child(SectionLabel::new(SharedString::from(label)))
}

fn prose(text: &'static str, palette: &Palette) -> impl IntoElement {
    div()
        .text_color(palette.fg2)
        .line_height(relative(LINE_PROSE))
        .child(text)
}

fn row(
    title: &'static str,
    hint: &'static str,
    control: impl IntoElement,
    palette: &Palette,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(SPACE_8)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(SPACE_1)
                .child(div().text_color(palette.fg).child(title))
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .text_color(palette.mute)
                        .child(hint),
                ),
        )
        .child(div().flex().flex_none().child(control))
        .into_any_element()
}

impl Render for AppSettingsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = *cx.palette();
        let current = settings::current(cx).clone();
        let highlights = self.highlights(&current, &palette, cx);
        let appearance = Self::appearance(&current, &palette);
        let column = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(SETTINGS_COLUMN)
            .px(SETTINGS_INSET)
            .pb(SPACE_8)
            .child(appearance.children(highlights))
            .child(Self::refresh(&current, &palette, cx))
            .child(Self::notifications(&current, &palette, cx))
            .child(updates(&current, &palette, cx))
            .child(shortcuts(&palette));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.header(&palette))
            .child(
                div()
                    .id("app-settings")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(column),
            )
    }
}
