use flock_core::{Page, Status};
use gpui::{AnyElement, Context, Hsla, IntoElement, ParentElement, SharedString, Styled, div};

use super::{OverviewData, OverviewScreen, mono, row_link, section, section_head};
use crate::theme::{
    FONT_STATUS, OVERVIEW_FACT_CHEVRON, OVERVIEW_FACT_GLYPH, Palette, SPACE_2, SPACE_4, TEXT_BODY,
    TEXT_CONTROL, TEXT_GLYPH, TEXT_LABEL, TEXT_MONO,
};

const UNKNOWN: &str = "—";
const CHEVRON: &str = "›";
const NO_SERVER_VERSION: &str = "control plane version unknown";
const NO_ENDPOINT_SLICES: &str = "EndpointSlices are not readable";

struct Fact {
    id: &'static str,
    key: &'static str,
    sub: Option<String>,
    value: Option<usize>,
    status: Status,
    page: Page,
}

impl Fact {
    fn counted(id: &'static str, key: &'static str, count: usize, page: Page) -> Self {
        let status = if count == 0 { Status::Ok } else { Status::Warn };
        Self {
            id,
            key,
            sub: None,
            value: Some(count),
            status,
            page,
        }
    }

    fn unknown(id: &'static str, key: &'static str, reason: &str, page: Page) -> Self {
        Self {
            id,
            key,
            sub: Some(reason.to_owned()),
            value: None,
            status: Status::Offline,
            page,
        }
    }
}

fn colors(fact: &Fact, palette: &Palette) -> (Hsla, Hsla) {
    match (fact.value, fact.status) {
        (None, _) => (palette.mute, palette.mute),
        (Some(_), Status::Ok) => (palette.ok, palette.mute),
        (Some(_), status) => (palette.status(status), palette.status(status)),
    }
}

fn row(fact: Fact, palette: &Palette, cx: &mut Context<OverviewScreen>) -> AnyElement {
    let (glyph_color, value_color) = colors(&fact, palette);
    let glyph = fact.status.glyph();
    let sub = fact
        .sub
        .map(|sub| mono(sub, palette.mute).text_size(TEXT_LABEL).truncate());
    let key = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(div().child(fact.key))
        .children(sub);
    let value = fact
        .value
        .map_or_else(|| UNKNOWN.to_owned(), |count| count.to_string());
    let activate = OverviewScreen::navigate(fact.page, cx);
    row_link(SharedString::from(fact.id), activate, palette)
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
                .child(CHEVRON),
        )
        .into_any_element()
}

fn group(
    title: &str,
    facts: Vec<Fact>,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let rows: Vec<AnyElement> = facts.into_iter().map(|f| row(f, palette, cx)).collect();
    section(palette)
        .child(section_head(title, None, None, palette))
        .children(rows)
        .into_any_element()
}

fn drift(data: &OverviewData) -> Vec<Fact> {
    let id = "overview-fact-kubelet-skew";
    let key = "Kubelet version skew";
    let skew = data.skew.as_ref().map_or_else(
        || Fact::unknown(id, key, NO_SERVER_VERSION, Page::Nodes),
        |skew| Fact {
            sub: Some(skew.detail()),
            status: skew.status(),
            ..Fact::counted(id, key, skew.nodes, Page::Nodes)
        },
    );
    vec![skew]
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
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> Vec<AnyElement> {
    vec![
        group("Version drift", drift(data), palette, cx),
        group("Network & storage", network(data), palette, cx),
    ]
}
