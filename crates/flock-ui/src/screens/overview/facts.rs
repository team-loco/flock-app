use flock_core::expiry::{Expiring, Horizon, SOON_DAYS, SupportEnd};
use flock_core::{Page, ResourceRef, Status};
use gpui::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div,
};

use super::{OverviewData, OverviewScreen, mono, row_link, section, section_head};
use crate::kit::Segmented;
use crate::theme::{
    FONT_STATUS, OVERVIEW_FACT_CHEVRON, OVERVIEW_FACT_GLYPH, Palette, RULE, SPACE_2, SPACE_3,
    SPACE_4, TEXT_BODY, TEXT_CONTROL, TEXT_GLYPH, TEXT_LABEL, TEXT_MONO, TEXT_SMALL,
};

const UNKNOWN: &str = "—";
const CHEVRON: &str = "›";
const NO_SERVER_VERSION: &str = "control plane version unknown";
const NO_ENDPOINT_SLICES: &str = "EndpointSlices are not readable";
const EXPIRING_TITLE: &str = "Expiring & drift";
const WITHIN: &str = "within";
const HORIZON: &str = "overview-expiring-horizon";
const EXPIRED: &str = "expired";
const ENDED: &str = "ended";
const SHOWN_CERTIFICATES: usize = 4;

enum Value {
    Count(usize),
    Days(i64, &'static str),
    Unknown,
    Blank,
}

enum Action {
    Navigate(Page),
    Open(ResourceRef),
    Inert,
}

struct Fact {
    id: String,
    key: String,
    sub: Option<String>,
    glyph: Option<Status>,
    value: Value,
    action: Action,
}

impl Fact {
    fn counted(id: &str, key: &str, count: usize, page: Page) -> Self {
        let status = if count == 0 { Status::Ok } else { Status::Warn };
        Self {
            id: id.to_owned(),
            key: key.to_owned(),
            sub: None,
            glyph: Some(status),
            value: Value::Count(count),
            action: Action::Navigate(page),
        }
    }

    fn unknown(id: &str, key: &str, reason: &str, page: Page) -> Self {
        Self {
            id: id.to_owned(),
            key: key.to_owned(),
            sub: Some(reason.to_owned()),
            glyph: Some(Status::Offline),
            value: Value::Unknown,
            action: Action::Navigate(page),
        }
    }

    fn certificate(ix: usize, item: &Expiring) -> Self {
        let value = item
            .days
            .map_or(Value::Unknown, |days| Value::Days(days, EXPIRED));
        let action = item.target.clone().map_or(Action::Inert, Action::Open);
        Self {
            id: format!("overview-expiring-{ix}"),
            key: item.label.clone(),
            sub: Some(item.detail.clone()),
            glyph: Some(item.status()),
            value,
            action,
        }
    }

    fn support(end: &SupportEnd) -> Self {
        Self {
            id: "overview-fact-support".to_owned(),
            key: end.label.clone(),
            sub: Some(end.date.to_string()),
            glyph: Some(end.status()),
            value: Value::Days(end.days, ENDED),
            action: Action::Inert,
        }
    }

    fn note(id: &str, key: String, glyph: Option<Status>) -> Self {
        Self {
            id: id.to_owned(),
            key,
            sub: None,
            glyph,
            value: Value::Blank,
            action: Action::Inert,
        }
    }
}

fn value_color(value: &Value, glyph: Option<Status>, palette: &Palette) -> Hsla {
    match value {
        Value::Count(_) => match glyph {
            Some(Status::Ok) => palette.mute,
            Some(status) => palette.status(status),
            None => palette.mute,
        },
        Value::Days(days, _) if *days < 0 => palette.err,
        Value::Days(days, _) if *days <= SOON_DAYS => palette.warn,
        Value::Days(_, _) => palette.fg2,
        Value::Unknown => palette.mute,
        Value::Blank => palette.mute,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Count(count) => count.to_string(),
        Value::Days(days, past) if *days < 0 => (*past).to_owned(),
        Value::Days(days, _) => format!("{days}d"),
        Value::Unknown => UNKNOWN.to_owned(),
        Value::Blank => String::new(),
    }
}

fn wrap(fact: &Fact, body: Div, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let id = SharedString::from(fact.id.clone());
    match &fact.action {
        Action::Navigate(page) => {
            let activate = OverviewScreen::navigate(*page, cx);
            row_link(id, activate, palette)
                .child(body)
                .into_any_element()
        }
        Action::Open(target) => {
            let activate = OverviewScreen::open(target.clone(), cx);
            row_link(id, activate, palette)
                .child(body)
                .into_any_element()
        }
        Action::Inert => {
            let selector = fact.id.clone();
            div()
                .debug_selector(|| selector)
                .border_t(RULE)
                .border_color(palette.line)
                .child(body)
                .into_any_element()
        }
    }
}

fn row(fact: &Fact, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let glyph_color = fact.glyph.map_or(palette.mute, |s| palette.status(s));
    let value_color = value_color(&fact.value, fact.glyph, palette);
    let glyph = fact.glyph.map(Status::glyph).unwrap_or_default();
    let key_color = if fact.glyph.is_some() {
        palette.fg
    } else {
        palette.mute
    };
    let sub = fact
        .sub
        .clone()
        .map(|sub| mono(sub, palette.mute).text_size(TEXT_LABEL).truncate());
    let key = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(div().text_color(key_color).child(fact.key.clone()))
        .children(sub);
    let chevron = match fact.action {
        Action::Navigate(_) => Some(CHEVRON),
        Action::Open(_) => Some(CHEVRON),
        Action::Inert => None,
    };
    let value = value_text(&fact.value);
    let body = div()
        .flex()
        .items_baseline()
        .gap(SPACE_4)
        .py(SPACE_2)
        .text_size(TEXT_BODY)
        .child(
            div()
                .flex_none()
                .w(OVERVIEW_FACT_GLYPH)
                .font_family(FONT_STATUS)
                .text_size(TEXT_GLYPH)
                .text_color(glyph_color)
                .child(glyph),
        )
        .child(key)
        .child(mono(value, value_color).text_size(TEXT_MONO))
        .child(
            div()
                .flex_none()
                .w(OVERVIEW_FACT_CHEVRON)
                .text_size(TEXT_CONTROL)
                .text_color(palette.sep)
                .children(chevron),
        );
    wrap(fact, body, palette, cx)
}

fn group(
    head: Div,
    facts: Vec<Fact>,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let rows: Vec<AnyElement> = facts.into_iter().map(|f| row(&f, palette, cx)).collect();
    section(palette)
        .child(head)
        .children(rows)
        .into_any_element()
}

fn horizon_control(horizon: Horizon, palette: &Palette, cx: &Context<OverviewScreen>) -> Div {
    let selected = Horizon::ALL
        .iter()
        .position(|h| *h == horizon)
        .unwrap_or_default();
    let screen = cx.entity().downgrade();
    let control = Horizon::ALL
        .iter()
        .fold(Segmented::new(HORIZON).small().mono(), |seg, h| {
            seg.option(h.label())
        })
        .selected(selected)
        .on_select(move |ix, _, cx| {
            let picked = Horizon::ALL.get(*ix).copied();
            if let Some(horizon) = picked {
                screen
                    .update(cx, |screen, cx| screen.select_horizon(horizon, cx))
                    .ok();
            }
        });
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(
            div()
                .text_size(TEXT_SMALL)
                .text_color(palette.mute)
                .child(WITHIN),
        )
        .child(control)
}

fn skew(data: &OverviewData) -> Fact {
    let id = "overview-fact-kubelet-skew";
    let key = "Kubelet version skew";
    data.skew.as_ref().map_or_else(
        || Fact::unknown(id, key, NO_SERVER_VERSION, Page::Nodes),
        |skew| Fact {
            sub: Some(skew.detail()),
            glyph: Some(skew.status()),
            ..Fact::counted(id, key, skew.nodes, Page::Nodes)
        },
    )
}

fn expiring(data: &OverviewData, horizon: Horizon) -> Vec<Fact> {
    let shown: Vec<&Expiring> = data
        .expiring
        .iter()
        .filter(|item| item.shown(horizon))
        .collect();
    let mut facts: Vec<Fact> = shown
        .iter()
        .take(SHOWN_CERTIFICATES)
        .enumerate()
        .map(|(ix, item)| Fact::certificate(ix, item))
        .collect();
    let more = shown.len().saturating_sub(SHOWN_CERTIFICATES);
    if more > 0 {
        let key = format!("+{more} more certificates");
        facts.push(Fact::note("overview-expiring-more", key, None));
    }
    let support = data.support.as_ref().filter(|end| end.shown(horizon));
    facts.extend(support.map(Fact::support));
    if facts.is_empty() {
        let key = format!("Nothing expires within {}", horizon.label());
        facts.push(Fact::note("overview-expiring-none", key, Some(Status::Ok)));
    }
    facts.push(skew(data));
    facts
}

fn network(data: &OverviewData) -> Vec<Fact> {
    let network = data.network;
    let endpoints_id = "overview-fact-services-without-endpoints";
    let endpoints_key = "Services without endpoints";
    let endpoints = network.services_without_endpoints.map_or_else(
        || {
            Fact::unknown(
                endpoints_id,
                endpoints_key,
                NO_ENDPOINT_SLICES,
                Page::Services,
            )
        },
        |count| Fact::counted(endpoints_id, endpoints_key, count, Page::Services),
    );
    vec![
        endpoints,
        Fact::counted(
            "overview-fact-load-balancers-pending",
            "LoadBalancers waiting for IP",
            network.load_balancers_pending,
            Page::Services,
        ),
        Fact::counted(
            "overview-fact-pvcs-pending",
            "PVCs Pending",
            network.pvcs_pending,
            Page::Pvcs,
        ),
    ]
}

pub(super) fn render(
    data: &OverviewData,
    horizon: Horizon,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> Vec<AnyElement> {
    let expiring_head = section_head(EXPIRING_TITLE, None, None, palette)
        .child(horizon_control(horizon, palette, cx));
    let network_head = section_head("Network & storage", None, None, palette);
    vec![
        group(expiring_head, expiring(data, horizon), palette, cx),
        group(network_head, network(data), palette, cx),
    ]
}
