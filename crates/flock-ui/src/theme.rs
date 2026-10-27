use std::time::Duration;

use flock_core::app_settings::{Highlight, ThemeChoice};
use flock_core::{AppSettings, Status};
use gpui::{App, Global, Hsla, Pixels, Window, WindowAppearance, px, rgb, rgba};

const TITLE_ROW_HEIGHT: f32 = 32.;
const TRAFFIC_LIGHT_LEFT: f32 = 10.;
const TRAFFIC_LIGHT_SIZE: f32 = 14.;
const TRAFFIC_LIGHT_GAP: f32 = 9.;
const TRAFFIC_LIGHT_COUNT: f32 = 3.;
const TRAFFIC_LIGHT_CLEARANCE: f32 = 8.;

const ANSI_BASE: [u32; 16] = [
    0x001d_2027,
    0x00c5_3030,
    0x0016_7a4c,
    0x0093_5800,
    0x001f_5fd1,
    0x00a3_47ba,
    0x0014_8a99,
    0x00b3_b9c3,
    0x005f_6774,
    0x00f2_6d6d,
    0x0048_c78e,
    0x00e8_b04b,
    0x0074_adff,
    0x00d3_8ce5,
    0x0067_d5df,
    0x00f2_f6fc,
];
const ANSI_CUBE_START: u8 = 16;
const ANSI_GRAY_START: u8 = 232;
const ANSI_CUBE_SIDE: u32 = 6;
const ANSI_CUBE_LOW: u32 = 55;
const ANSI_CUBE_STEP: u32 = 40;
const ANSI_GRAY_LOW: u32 = 8;
const ANSI_GRAY_STEP: u32 = 10;
const RED_SHIFT: u32 = 16;
const GREEN_SHIFT: u32 = 8;

pub const FONT_SANS: &str = "IBM Plex Sans";
pub const FONT_STATUS: &str = "Flock Status";
pub const FONT_MONO: &str = "IBM Plex Mono";
pub const MONO_ADVANCE: f32 = 0.6;

pub const TEXT_UI: Pixels = px(13.);
pub const TEXT_MONO: Pixels = px(12.);
pub const TEXT_LABEL: Pixels = px(11.);
pub const TEXT_SMALL: Pixels = px(11.5);
pub const TEXT_GLYPH: Pixels = px(10.);
pub const TEXT_TITLE: Pixels = px(14.);
pub const TEXT_BODY: Pixels = px(12.5);
pub const TEXT_CONTROL: Pixels = px(12.);
pub const TEXT_AXIS: Pixels = px(10.5);
pub const TEXT_HEADING: Pixels = px(16.);
pub const TEXT_SUBHEADING: Pixels = px(15.);
pub const TEXT_FIGURE: Pixels = px(26.);
pub const LINE_PROSE: f32 = 1.5;
pub const LABEL_TRACKING: f32 = 0.07;

pub const RADIUS: Pixels = px(3.);
pub const RADIUS_MENU: Pixels = px(4.);
pub const RADIUS_SMALL: Pixels = px(2.);
pub const RULE: Pixels = px(1.);

pub const ROW_TABLE_MIN: Pixels = px(24.);
pub const ROW_TABLE_MAX: Pixels = px(28.);
pub const ROW_TABLE: Pixels = px(26.);
pub const ROW_DOCUMENT: Pixels = px(25.);
pub const ROW_MENU_PAD_Y: Pixels = px(6.);
pub const ROW_NAV: Pixels = px(26.);
pub const ROW_CLUSTER: Pixels = px(28.);
pub const ROW_COMMIT: Pixels = px(48.);
pub const ROW_ROLLOUT: Pixels = px(25.);
pub const ROW_MATRIX: Pixels = px(38.);
pub const ROW_CATALOG: Pixels = px(44.);

pub const TITLE_ROW: Pixels = px(TITLE_ROW_HEIGHT);
pub const SIDEBAR: Pixels = px(200.);
pub const SIDEBAR_MIN: Pixels = px(SIDEBAR_MIN_WIDTH);
pub const SIDEBAR_MAX: Pixels = px(360.);
pub const SIDEBAR_CLOSE: Pixels = px(SIDEBAR_MIN_WIDTH / 2.);
pub const SWITCHER_WIDTH: Pixels = px(290.);
pub const SWITCHER_GEAR: Pixels = px(26.);
pub const SWITCHER_LIST: Pixels = px(420.);
pub const SWITCHER_MENU: Pixels = px(190.);
pub const PAGE_HEADER: Pixels = px(44.);
pub const TAB_CLOSE: Pixels = px(14.);
pub const SPLIT_BUTTON: Pixels = px(32.);
pub const ICON_SPLIT: Pixels = px(14.);
pub const ICON_LINK: Pixels = px(14.);
pub const STATUS_BAR: Pixels = px(26.);
pub const STATUS_REFRESH: Pixels = px(32.);
pub const ASIDE_WIDTH: Pixels = px(300.);
pub const SETTINGS_COLUMN: Pixels = px(720.);
pub const SETTINGS_INSET: Pixels = SPACE_8;
pub const SETTINGS_FIELD: Pixels = px(300.);

const SIDEBAR_MIN_WIDTH: f32 = 160.;
const ICON_SIZE: f32 = 11.;
const ICON_BUTTON_SIZE: f32 = 22.;
pub const ICON: Pixels = px(ICON_SIZE);
pub const ICON_BUTTON: Pixels = px(ICON_BUTTON_SIZE);
pub const ICON_BUTTON_INSET: Pixels = px((ICON_BUTTON_SIZE - ICON_SIZE) / 2.);
pub const GLYPH_SLOT: Pixels = px(10.);
pub const CONTAINER_SQUARE: Pixels = px(8.);
pub const CONTAINER_SQUARE_GAP: Pixels = px(3.);
pub const TRAFFIC_LIGHT_X: Pixels = px(TRAFFIC_LIGHT_LEFT);
pub const TRAFFIC_LIGHT_Y: Pixels = px((TITLE_ROW_HEIGHT - TRAFFIC_LIGHT_SIZE) / 2.);
pub const TRAFFIC_LIGHTS_INSET: Pixels = px(TRAFFIC_LIGHT_LEFT
    + TRAFFIC_LIGHT_COUNT * TRAFFIC_LIGHT_SIZE
    + (TRAFFIC_LIGHT_COUNT - 1.) * TRAFFIC_LIGHT_GAP
    + TRAFFIC_LIGHT_CLEARANCE);

pub const SPACE_1: Pixels = px(2.);
pub const SPACE_2: Pixels = px(4.);
pub const SPACE_3: Pixels = px(6.);
pub const SPACE_4: Pixels = px(8.);
pub const SPACE_5: Pixels = px(10.);
pub const SPACE_6: Pixels = px(12.);
pub const SPACE_7: Pixels = px(14.);
pub const SPACE_8: Pixels = px(16.);

pub const CONTROL: Pixels = px(24.);
pub const CONTROL_SMALL: Pixels = px(20.);
pub const CONTROL_COMPACT: Pixels = px(22.);
pub const SEGMENT: Pixels = px(22.);
pub const MORE_BUTTON: Pixels = px(28.);
pub const SEARCH_WIDTH: Pixels = px(180.);
pub const FILTER_WIDTH: Pixels = px(390.);
pub const FILTER_MIN: Pixels = px(240.);
pub const MATCH_NOTE: Pixels = px(96.);
pub const COLUMN_MIN: Pixels = px(32.);
pub const COLUMN_HANDLE: Pixels = px(8.);
pub const COLUMN_DIVIDER: Pixels = px(12.);
pub const MENU_WIDTH: Pixels = px(300.);
pub const CHECK_SLOT: Pixels = px(14.);
pub const MENU_MAX_HEIGHT: Pixels = px(360.);
pub const DIALOG_WIDTH: Pixels = px(440.);
pub const DIALOG_TOP: Pixels = px(84.);
pub const TOAST_WIDTH: Pixels = px(320.);
pub const SHADOW_OFFSET: Pixels = px(12.);
pub const SHADOW_BLUR: Pixels = px(32.);
pub const POPOVER_SLIDE: Pixels = px(4.);
pub const TOOLTIP_MAX_WIDTH: Pixels = px(280.);
pub const PALETTE_WIDTH: Pixels = px(680.);
pub const PALETTE_TOP: Pixels = px(64.);
pub const PALETTE_INPUT: Pixels = px(40.);
pub const PALETTE_RESULTS: Pixels = px(560.);
pub const PALETTE_KIND: Pixels = px(52.);
pub const PALETTE_NAMESPACE: Pixels = px(92.);
pub const PALETTE_CLUSTER: Pixels = px(104.);
pub const PALETTE_STATUS: Pixels = px(132.);
pub const PALETTE_SCALE_FROM: f32 = 0.98;
pub const TEXT_PALETTE: Pixels = px(14.);

pub const TOGGLE_WIDTH: Pixels = px(28.);
pub const TOGGLE_HEIGHT: Pixels = px(16.);
pub const TOGGLE_KNOB: Pixels = px(12.);
pub const CHECKBOX: Pixels = px(14.);
pub const SWATCH: Pixels = px(20.);
pub const HEX_FIELD: Pixels = px(96.);
pub const CHECKBOX_TICK: Pixels = px(10.);

pub const KV_LABEL: Pixels = px(72.);
pub const FIELD_LABEL: Pixels = px(150.);
pub const INSPECTOR: Pixels = px(360.);
pub const INSPECTOR_NODE: Pixels = px(420.);
pub const INSPECTOR_RESOURCE: Pixels = px(480.);
pub const INSPECTOR_POD: Pixels = px(360.);
pub const MESSAGE_MIN: Pixels = px(100.);
pub const INSPECTOR_WORKLOAD: Pixels = px(560.);
pub const CONDITION_VALUE: Pixels = px(56.);
pub const CONDITION_AGE: Pixels = px(40.);
pub const REASON_MIN: Pixels = px(240.);
pub const REASON_WEIGHT: f32 = 2.;
pub const ERROR_BAR: Pixels = px(3.);
pub const SECTION_HEADER: Pixels = px(30.);
pub const PANEL_HEADER: Pixels = px(40.);
pub const SECTION_INDENT: Pixels = px(18.);
pub const EVENT_AGE: Pixels = px(28.);
pub const INSPECTOR_MIN: Pixels = px(280.);
pub const INSPECTOR_MAX: Pixels = px(900.);
pub const CODE_HEIGHT: Pixels = px(400.);
pub const DOCK_HEIGHT: Pixels = px(250.);
pub const DOCK_BAR: Pixels = px(30.);
pub const SPLITTER: Pixels = px(5.);
pub const SPLIT_MIN: Pixels = px(160.);
pub const LOG_LINE: Pixels = px(19.);
pub const LOG_LEVEL: Pixels = px(40.);
pub const LOG_TIMESTAMP: Pixels = px(96.);
pub const LOG_SOURCE: Pixels = px(132.);
pub const LEGEND_CHIP_SWATCH: Pixels = px(8.);
pub const CODE_LINE: Pixels = px(20.);
pub const GUTTER_NUMBER: Pixels = px(40.);
pub const GUTTER_SIGN: Pixels = px(16.);
pub const DIFF_HEADER: Pixels = px(28.);
pub const DIFF_NOTE_MIN: Pixels = px(160.);
pub const TERMINAL_CURSOR_WIDTH: Pixels = px(7.);
pub const TERMINAL_CURSOR_HEIGHT: Pixels = px(15.);

pub const METER: Pixels = px(6.);
pub const METER_MINI: Pixels = px(4.);
pub const METER_MINI_WIDTH: Pixels = px(36.);
pub const METER_MARKER: Pixels = px(12.);
pub const METER_MARKER_WIDTH: Pixels = px(2.);
pub const CHART_HEIGHT: Pixels = px(170.);
pub const CHART_AXIS: Pixels = px(30.);
pub const CHART_STROKE: Pixels = px(2.);
pub const CHART_DASH: Pixels = px(4.);
pub const LEGEND_SWATCH: Pixels = px(10.);
pub const GRAPH_NODE_WIDTH: Pixels = px(220.);
pub const GRAPH_NODE_HEIGHT: Pixels = px(44.);
pub const GRAPH_COLUMN_GAP: Pixels = px(72.);
pub const GRAPH_ROW_GAP: Pixels = px(14.);
pub const GRAPH_EDGE: Pixels = px(1.);
pub const GRAPH_EDGE_ACTIVE: Pixels = px(1.5);
pub const GRAPH_LEGEND_SWATCH: Pixels = px(8.);
pub const GRAPH_COMPARE_COLUMN: Pixels = px(180.);
pub const ALLOCATION_HEIGHT: Pixels = px(330.);
pub const ALLOCATION_HEADER: Pixels = px(34.);
pub const ALLOCATION_SIDE: Pixels = px(250.);
pub const ALLOCATION_CURRENT: Pixels = px(74.);
pub const TILE_BORDER: Pixels = px(1.5);
pub const TILE_FOCUS: Pixels = px(2.);
pub const TILE_LABEL_X: Pixels = px(5.);
pub const TILE_LABEL_Y: Pixels = px(3.);
pub const TILE_LABEL_LINE: Pixels = px(13.);
pub const TILE_LABEL_MIN_WIDTH: f32 = 58.;
pub const TILE_LABEL_MIN_HEIGHT: f32 = 30.;
pub const FREE_LABEL_MIN_WIDTH: f32 = 70.;
pub const FREE_LABEL_MIN_HEIGHT: f32 = 18.;
pub const TILE_DIM: f32 = 0.45;
pub const TILE_TINT: f32 = 0.38;
pub const TILE_AMOUNT_OPACITY: f32 = 0.85;
pub const SIDECAR_STRIPE: f32 = 3.;
pub const FREE_HATCH_WIDTH: f32 = 1.;
pub const FREE_HATCH_INTERVAL: f32 = 4.;
pub const CATEGORIES: usize = flock_core::allocation::NAMESPACE_SLOTS;

pub const DIALOG_WIDE: Pixels = px(720.);
pub const PICK_CHECK_COLUMN: Pixels = px(20.);
pub const PICK_NAME_COLUMN: Pixels = px(180.);
pub const PASTE_AREA_HEIGHT: Pixels = px(260.);
pub const PICK_ROW_PAD_Y: Pixels = px(7.);
pub const CHECK_GLYPH_COLUMN: Pixels = px(14.);
pub const CODE_PAD_Y: Pixels = px(3.);
pub const EMPTY_WIDTH: Pixels = px(520.);

pub const COLLAPSE: Duration = Duration::from_millis(150);
pub const INSPECTOR_IN: Duration = Duration::from_millis(180);
pub const INSPECTOR_OUT: Duration = Duration::from_millis(140);
pub const INSPECTOR_SWAP: Duration = Duration::from_millis(140);
pub const INSPECTOR_SLIDE: Pixels = px(12.);
pub const TAB_ENTER: Duration = Duration::from_millis(150);
pub const TAB_LEAVE: Duration = TAB_ENTER;
pub const TAB_SETTLE: Duration = Duration::from_millis(100);
pub const TAB_SETTLE_SLIDE: Pixels = px(24.);
pub const TAB_GROW_MAX: Pixels = px(240.);
pub const POPOVER_IN: Duration = Duration::from_millis(120);
pub const PALETTE_IN: Duration = Duration::from_millis(140);
pub const PALETTE_OUT: Duration = Duration::from_millis(90);
pub const FLASH: Duration = Duration::from_millis(900);
pub const TOAST_SUCCESS_TIMEOUT: Duration = Duration::from_secs(4);
pub const TOAST_WARNING_TIMEOUT: Duration = Duration::from_secs(8);
pub const TOAST_PROGRESS_TIMEOUT: Duration = Duration::from_secs(30);
pub const SKELETON_SHIMMER: Duration = Duration::from_millis(1800);
pub const SKELETON_SHIMMER_OPACITY: f32 = 0.35;
pub const SKELETON_GRADIENT_ANGLE: f32 = 90.;
pub const SKELETON_GRADIENT_START: f32 = 0.;
pub const SKELETON_GRADIENT_END: f32 = 1.;
pub const SKELETON_SHIMMER_WIDTH: Pixels = px(80.);
pub const SKELETON_ROWS: usize = 8;
pub const SKELETON_NAME_WIDTH: Pixels = px(220.);
pub const SKELETON_NAMESPACE_WIDTH: Pixels = px(130.);
pub const SKELETON_STATUS_WIDTH: Pixels = px(300.);
pub const SKELETON_TITLE_WIDTH: Pixels = px(240.);
pub const SKELETON_METER_WIDTH: Pixels = px(260.);
pub const SKELETON_METER_HEIGHT: Pixels = px(64.);
pub const SKELETON_CHART_WIDTH: Pixels = px(640.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub fn from_window(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark => Self::Dark,
            WindowAppearance::VibrantDark => Self::Dark,
            WindowAppearance::Light => Self::Light,
            WindowAppearance::VibrantLight => Self::Light,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub editor: Hsla,
    pub panel: Hsla,
    pub chrome: Hsla,
    pub hover: Hsla,
    pub line: Hsla,
    pub line2: Hsla,
    pub fg: Hsla,
    pub fg2: Hsla,
    pub mute: Hsla,
    pub edge: Hsla,
    pub sel: Hsla,
    pub selfg: Hsla,
    pub selline: Hsla,
    pub textsel_on_sel: Hsla,
    pub acc: Hsla,
    pub onacc: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    pub err: Hsla,
    pub sus: Hsla,
    pub okbg: Hsla,
    pub warnbg: Hsla,
    pub errbg: Hsla,
    pub raise: Hsla,
    pub guide: Hsla,
    pub sep: Hsla,
    pub accbg: Hsla,
    pub ai: Hsla,
    pub addbg: Hsla,
    pub delbg: Hsla,
    pub addfg: Hsla,
    pub delfg: Hsla,
    pub shadow: Hsla,
    pub scrim: Hsla,
    pub categorical: [Hsla; CATEGORIES],
    pub on_tile: Hsla,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            editor: rgb(0x0011_1317).into(),
            panel: rgb(0x0015_181D).into(),
            chrome: rgb(0x001A_1D22).into(),
            hover: rgb(0x001D_2127).into(),
            line: rgb(0x0026_2A30).into(),
            line2: rgb(0x0032_3740).into(),
            fg: rgb(0x00DC_DFE4).into(),
            fg2: rgb(0x00B3_B9C3).into(),
            mute: rgb(0x008E_96A3).into(),
            edge: rgb(0x0060_6876).into(),
            sel: rgb(0x001F_2C40).into(),
            selfg: rgb(0x00F2_F6FC).into(),
            selline: rgb(0x003A_5A8C).into(),
            textsel_on_sel: rgb(0x002A_3F5F).into(),
            acc: rgb(0x0074_ADFF).into(),
            onacc: rgb(0x000C_1220).into(),
            ok: rgb(0x0048_C78E).into(),
            warn: rgb(0x00E8_B04B).into(),
            err: rgb(0x00F2_6D6D).into(),
            sus: rgb(0x009A_A2AE).into(),
            okbg: rgba(0x48C7_8E1A).into(),
            warnbg: rgba(0xE8B0_4B1A).into(),
            errbg: rgba(0xF26D_6D1C).into(),
            raise: rgb(0x0020_242A).into(),
            guide: rgb(0x002A_2E35).into(),
            sep: rgb(0x004C_525C).into(),
            accbg: rgba(0x74AD_FF1F).into(),
            ai: rgb(0x00B4_8CFF).into(),
            addbg: rgba(0x48C7_8E1F).into(),
            delbg: rgba(0xF26D_6D21).into(),
            addfg: rgb(0x007F_DDB2).into(),
            delfg: rgb(0x00FF_9C9C).into(),
            shadow: rgba(0x0000_0080).into(),
            scrim: rgba(0x0405_078C).into(),
            categorical: [
                rgb(0x0039_87E5).into(),
                rgb(0x0019_9E70).into(),
                rgb(0x0090_85E9).into(),
                rgb(0x00D5_5181).into(),
                rgb(0x00D9_5926).into(),
                rgb(0x00C9_8500).into(),
            ],
            on_tile: rgb(0x00FF_FFFF).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            editor: rgb(0x00FF_FFFF).into(),
            panel: rgb(0x00F6_F7F8).into(),
            chrome: rgb(0x00EE_EFF1).into(),
            hover: rgb(0x00F1_F2F4).into(),
            line: rgb(0x00E1_E3E7).into(),
            line2: rgb(0x00CF_D3D9).into(),
            fg: rgb(0x001D_2027).into(),
            fg2: rgb(0x003B_414B).into(),
            mute: rgb(0x005F_6774).into(),
            edge: rgb(0x0086_8E9B).into(),
            sel: rgb(0x00DC_E7FA).into(),
            selfg: rgb(0x000D_2850).into(),
            selline: rgb(0x008D_B0EA).into(),
            textsel_on_sel: rgb(0x00BC_D0F3).into(),
            acc: rgb(0x001F_5FD1).into(),
            onacc: rgb(0x00FF_FFFF).into(),
            ok: rgb(0x0016_7A4C).into(),
            warn: rgb(0x0093_5800).into(),
            err: rgb(0x00C5_3030).into(),
            sus: rgb(0x006B_7280).into(),
            okbg: rgba(0x167A_4C14).into(),
            warnbg: rgba(0x9358_0014).into(),
            errbg: rgba(0xC530_3012).into(),
            raise: rgb(0x00E9_EBEE).into(),
            guide: rgb(0x00E4_E6EA).into(),
            sep: rgb(0x00A5_ABB5).into(),
            accbg: rgba(0x1F5F_D114).into(),
            ai: rgb(0x007C_4DDB).into(),
            addbg: rgba(0x167A_4C1A).into(),
            delbg: rgba(0xC530_3017).into(),
            addfg: rgb(0x000F_5E3A).into(),
            delfg: rgb(0x00A1_2626).into(),
            shadow: rgba(0x1418_202E).into(),
            scrim: rgba(0x1418_2038).into(),
            categorical: [
                rgb(0x002A_78D6).into(),
                rgb(0x001B_AF7A).into(),
                rgb(0x0062_50D6).into(),
                rgb(0x00E8_7BA4).into(),
                rgb(0x00EB_6834).into(),
                rgb(0x00C9_8500).into(),
            ],
            on_tile: rgb(0x00FF_FFFF).into(),
        }
    }

    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(),
            Appearance::Light => Self::light(),
        }
    }

    pub fn status(&self, status: Status) -> Hsla {
        match status {
            Status::Ok => self.ok,
            Status::Warn => self.warn,
            Status::Err => self.err,
            Status::Progress => self.acc,
            Status::Suspended => self.sus,
            Status::Offline => self.mute,
        }
    }

    pub fn tint(&self, status: Status) -> Option<Hsla> {
        match status {
            Status::Ok => Some(self.okbg),
            Status::Warn => Some(self.warnbg),
            Status::Err => Some(self.errbg),
            Status::Progress => None,
            Status::Suspended => None,
            Status::Offline => None,
        }
    }

    pub fn toast_ground(&self, status: Status) -> Hsla {
        let tint = match status {
            Status::Progress => Some(self.accbg),
            _ => self.tint(status),
        };
        tint.map_or(self.panel, |tint| crate::highlight::over(tint, self.panel))
    }
}

pub struct Theme {
    pub forced: Option<Appearance>,
    pub choice: ThemeChoice,
    pub appearance: Appearance,
    pub primary: Highlight,
    pub secondary: Highlight,
    pub palette: Palette,
}

impl Global for Theme {}

impl Theme {
    fn derive(&self) -> Palette {
        let base = Palette::for_appearance(self.appearance);
        crate::highlight::apply(base, self.appearance, self.primary, self.secondary)
    }
}

pub trait ActiveTheme {
    fn palette(&self) -> &Palette;
}

impl ActiveTheme for App {
    fn palette(&self) -> &Palette {
        &self.global::<Theme>().palette
    }
}

pub fn resolve(forced: Option<Appearance>, choice: ThemeChoice, os: Appearance) -> Appearance {
    let chosen = match choice {
        ThemeChoice::System => os,
        ThemeChoice::Light => Appearance::Light,
        ThemeChoice::Dark => Appearance::Dark,
    };
    forced.unwrap_or(chosen)
}

pub fn init(forced: Option<Appearance>, cx: &mut App) {
    let choice = ThemeChoice::default();
    let os = Appearance::from_window(cx.window_appearance());
    let appearance = resolve(forced, choice, os);
    let palette = Palette::for_appearance(appearance);
    cx.set_global(Theme {
        forced,
        choice,
        appearance,
        primary: Highlight::default(),
        secondary: Highlight::default(),
        palette,
    });
    sync_component_theme(appearance, &palette, cx);
}

pub fn follow_window(window: &Window, cx: &mut App) {
    let os = Appearance::from_window(window.appearance());
    show(os, cx);
}

pub fn configure(settings: &AppSettings, cx: &mut App) {
    let theme = cx.global_mut::<Theme>();
    theme.choice = settings.theme;
    theme.primary = settings.primary;
    theme.secondary = settings.secondary;
    let os = Appearance::from_window(cx.window_appearance());
    if show(os, cx) {
        cx.refresh_windows();
    }
}

fn show(os: Appearance, cx: &mut App) -> bool {
    let theme = cx.global::<Theme>();
    let appearance = resolve(theme.forced, theme.choice, os);
    let mut next = Theme {
        appearance,
        ..*theme
    };
    next.palette = next.derive();
    if next.appearance == theme.appearance && next.palette == theme.palette {
        return false;
    }
    let palette = next.palette;
    cx.set_global(next);
    sync_component_theme(appearance, &palette, cx);
    true
}

fn sync_component_theme(appearance: Appearance, palette: &Palette, cx: &mut App) {
    let mode = match appearance {
        Appearance::Dark => gpui_component::ThemeMode::Dark,
        Appearance::Light => gpui_component::ThemeMode::Light,
    };
    gpui_component::Theme::change(mode, None, cx);
    let palette = *palette;
    gpui_component::Theme::update(cx, |theme| {
        theme.font_family = FONT_SANS.into();
        theme.mono_font_family = FONT_MONO.into();
        theme.font_size = TEXT_UI;
        theme.mono_font_size = TEXT_MONO;
        theme.radius = RADIUS;
        theme.radius_lg = RADIUS_MENU;
        theme.shadow = false;
        theme.background = palette.editor;
        theme.foreground = palette.fg;
        theme.muted_foreground = palette.mute;
        theme.border = palette.line;
        theme.input = palette.line2;
        theme.primary = palette.acc;
        theme.primary_foreground = palette.onacc;
        theme.accent = palette.hover;
        theme.selection = palette.sel;
        theme.popover = palette.panel;
        theme.popover_foreground = palette.fg;
        theme.danger = palette.err;
        theme.warning = palette.warn;
        theme.success = palette.ok;
    });
}

pub const EVENT_REASON_WIDTH: Pixels = px(190.);
pub const EVENT_OBJECT_WEIGHT: f32 = 1.2;
pub const EVENT_OBJECT_MIN: Pixels = px(290.);
pub const EVENT_NAMESPACE_WIDTH: Pixels = px(110.);
pub const EVENT_MESSAGE_WEIGHT: f32 = 2.;
pub const EVENT_MESSAGE_MIN: Pixels = px(260.);
pub const EVENT_COUNT_WIDTH: Pixels = px(56.);
pub const EVENT_AGE_WIDTH: Pixels = px(44.);
pub const FLUX_EVENT_CLUSTER_WIDTH: Pixels = px(130.);

pub const NAMESPACE_CONTENT_NAME: Pixels = px(220.);
pub const NAMESPACE_CONTENT_AGE: Pixels = px(60.);
pub const NAMESPACE_FILTER_WIDTH: Pixels = px(200.);
pub const NAMESPACE_USAGE_LABEL: Pixels = px(70.);
pub const NAMESPACE_USAGE_BAR: Pixels = px(4.);

pub const PROBLEM_CAUSES_WIDTH: Pixels = px(360.);
pub const PROBLEM_HISTOGRAM: Pixels = px(84.);
pub const PROBLEM_BAR_GAP: Pixels = px(2.);
pub const PROBLEM_BAR_EMPTY: Pixels = px(1.);
pub const PROBLEM_TRAIT_NAME: Pixels = px(150.);
pub const PROBLEM_TRAIT_BAR: Pixels = px(140.);
pub const PROBLEM_TRAIT_BAR_HEIGHT: Pixels = px(5.);
pub const PROBLEM_TRAIT_SHARE: Pixels = px(64.);
pub const AFFECTED_NAMESPACE_WIDTH: Pixels = px(110.);
pub const AFFECTED_WORKLOAD_WIDTH: Pixels = px(150.);
pub const AFFECTED_OBJECT_MIN: Pixels = px(180.);
pub const AFFECTED_NODE_WIDTH: Pixels = px(130.);
pub const AFFECTED_TAG_WIDTH: Pixels = px(70.);
pub const AFFECTED_SINCE_WIDTH: Pixels = px(50.);

pub const OVERVIEW_CHANGE_TIME: Pixels = px(44.);
pub const OVERVIEW_CHANGE_KIND: Pixels = px(74.);
pub const OVERVIEW_CHANGE_BY: Pixels = px(110.);
pub const OVERVIEW_CHANGE_ROW: Pixels = px(25.);
pub const OVERVIEW_NAMESPACE_MIN: Pixels = px(110.);
pub const OVERVIEW_COUNT_WIDTH: Pixels = px(64.);
pub const OVERVIEW_USAGE_MIN: Pixels = px(150.);
pub const OVERVIEW_USAGE_WEIGHT: f32 = 1.3;
pub const OVERVIEW_RESTARTS_WIDTH: Pixels = px(80.);
pub const OVERVIEW_NAMESPACE_ROW: Pixels = px(34.);
pub const OVERVIEW_USAGE_BAR: Pixels = px(3.);
pub const OVERVIEW_WORKLOAD_KIND: Pixels = px(120.);
pub const OVERVIEW_WORKLOAD_OK: Pixels = px(110.);
pub const OVERVIEW_WORKLOAD_BAD: Pixels = px(120.);
pub const OVERVIEW_WORKLOAD_ROW: Pixels = px(30.);
pub const OVERVIEW_CHART_HEIGHT: Pixels = px(110.);
pub const OVERVIEW_POOL_NODES: Pixels = px(34.);
pub const OVERVIEW_POOL_METER: Pixels = px(62.);
pub const OVERVIEW_POOL_FIT: Pixels = px(96.);
pub const OVERVIEW_POOL_ROW: Pixels = px(30.);
pub const OVERVIEW_FACT_GLYPH: Pixels = px(14.);
pub const OVERVIEW_FACT_CHEVRON: Pixels = px(10.);
pub const OVERVIEW_DRILL_LABEL: Pixels = px(72.);
pub const OVERVIEW_DRILL_BARS: Pixels = px(48.);
pub const OVERVIEW_DRILL_BAR_GAP: Pixels = px(1.);
pub const OVERVIEW_DRILL_CODE: Pixels = px(30.);

pub fn terminal_color(color: vt100::Color, default: Hsla, palette: &Palette) -> Hsla {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(red, green, blue) => {
            let value =
                (u32::from(red) << RED_SHIFT) | (u32::from(green) << GREEN_SHIFT) | u32::from(blue);
            rgb(value).into()
        }
        vt100::Color::Idx(index) => {
            if let Some(value) = ANSI_BASE.get(usize::from(index)) {
                return match index {
                    1 => palette.err,
                    2 => palette.ok,
                    3 => palette.warn,
                    4 => palette.acc,
                    7 => palette.fg2,
                    15 => palette.fg,
                    _ => rgb(*value).into(),
                };
            }
            let value = if index >= ANSI_GRAY_START {
                let shade = ANSI_GRAY_LOW + u32::from(index - ANSI_GRAY_START) * ANSI_GRAY_STEP;
                (shade << RED_SHIFT) | (shade << GREEN_SHIFT) | shade
            } else {
                let index = u32::from(index - ANSI_CUBE_START);
                let channel = |level| {
                    if level == 0 {
                        0
                    } else {
                        ANSI_CUBE_LOW + ANSI_CUBE_STEP * level
                    }
                };
                let red = channel(index / (ANSI_CUBE_SIDE * ANSI_CUBE_SIDE));
                let green = channel(index / ANSI_CUBE_SIDE % ANSI_CUBE_SIDE);
                let blue = channel(index % ANSI_CUBE_SIDE);
                (red << RED_SHIFT) | (green << GREEN_SHIFT) | blue
            };
            rgb(value).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_colors_support_theme_ansi_cube_gray_and_rgb() {
        for palette in [Palette::dark(), Palette::light()] {
            let color = |value| terminal_color(value, palette.fg, &palette);
            assert_eq!(color(vt100::Color::Default), palette.fg);
            assert_eq!(color(vt100::Color::Idx(1)), palette.err);
            let red: Hsla = rgb(0x00ff_0000).into();
            assert_eq!(color(vt100::Color::Idx(196)), red);
            let gray: Hsla = rgb(0x0008_0808).into();
            assert_eq!(color(vt100::Color::Idx(232)), gray);
            let custom: Hsla = rgb(0x0012_3456).into();
            assert_eq!(color(vt100::Color::Rgb(0x12, 0x34, 0x56)), custom);
        }
    }

    fn linear(channel: f32) -> f32 {
        if channel <= 0.03928 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(color: Hsla) -> f32 {
        let rgba = color.to_rgb();
        0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn graph_edges_reach_non_text_contrast_in_both_themes() {
        const MIN_GRAPHIC_CONTRAST: f32 = 3.0;
        for palette in [Palette::dark(), Palette::light()] {
            let ratio = contrast(palette.edge, palette.editor);
            assert!(ratio >= MIN_GRAPHIC_CONTRAST, "edge contrast {ratio}");
            assert!(ratio < contrast(palette.fg, palette.editor));
        }
    }

    #[test]
    fn status_colours_use_status_tokens() {
        let palette = Palette::dark();
        assert_eq!(palette.status(Status::Err), palette.err);
        assert_eq!(palette.status(Status::Offline), palette.mute);
    }

    #[test]
    fn the_flag_wins_then_the_choice_then_the_os() {
        let os = Appearance::Dark;
        assert_eq!(resolve(None, ThemeChoice::System, os), Appearance::Dark);
        assert_eq!(
            resolve(None, ThemeChoice::System, Appearance::Light),
            Appearance::Light
        );
        assert_eq!(resolve(None, ThemeChoice::Light, os), Appearance::Light);
        assert_eq!(
            resolve(Some(Appearance::Dark), ThemeChoice::Light, os),
            Appearance::Dark
        );
    }

    #[test]
    fn tinted_backgrounds_are_translucent() {
        for palette in [Palette::dark(), Palette::light()] {
            for tint in [palette.okbg, palette.warnbg, palette.errbg] {
                assert!(tint.a > 0.05 && tint.a < 0.15);
            }
        }
    }
}
