use flock_core::Status;
use gpui::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div};

use super::{OverviewData, OverviewScreen, mono, problem_link};
use crate::kit::{ClickableSection, SectionLabel, StackedBar};
use crate::theme::{
    OVERVIEW_WORKLOAD_BAD, OVERVIEW_WORKLOAD_KIND, OVERVIEW_WORKLOAD_OK, OVERVIEW_WORKLOAD_ROW,
    Palette, RULE, SPACE_4, SPACE_7, SPACE_8, TEXT_BODY, TEXT_MONO,
};

fn weight(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

pub(super) fn render(
    data: &OverviewData,
    palette: &Palette,
    cx: &mut Context<OverviewScreen>,
) -> AnyElement {
    let rows: Vec<AnyElement> = data
        .workloads
        .iter()
        .map(|w| {
            let page = w.page;
            let id = format!("overview-{}", page.label().to_lowercase());
            let bad_color = palette.status(w.bad_status);
            let bar = StackedBar::new(vec![
                (Status::Ok, weight(w.ok)),
                (w.bad_status, weight(w.bad)),
            ])
            .thin();
            let bad = mono(w.bad_label.clone().unwrap_or_default(), bad_color)
                .flex_none()
                .w(OVERVIEW_WORKLOAD_BAD)
                .text_size(TEXT_MONO);
            let bad = if w.bad > 0 {
                let activate = OverviewScreen::attention(page, None, cx);
                problem_link(format!("{id}-bad"), bad, activate).into_any_element()
            } else {
                bad.into_any_element()
            };
            let row = div()
                .flex()
                .items_center()
                .gap(SPACE_7)
                .h(OVERVIEW_WORKLOAD_ROW)
                .child(
                    div()
                        .flex_none()
                        .w(OVERVIEW_WORKLOAD_KIND)
                        .text_size(TEXT_BODY)
                        .child(page.label()),
                )
                .child(
                    mono(w.ok_label.clone(), palette.fg2)
                        .flex_none()
                        .w(OVERVIEW_WORKLOAD_OK)
                        .text_size(TEXT_MONO),
                )
                .child(bad)
                .child(div().flex_1().min_w_0().child(bar));
            let section = ClickableSection::new(SharedString::from(id))
                .padding(SPACE_8, gpui::Pixels::ZERO)
                .gap(gpui::Pixels::ZERO)
                .child(row)
                .on_activate(OverviewScreen::navigate(page, cx));
            div()
                .border_b(RULE)
                .border_color(palette.line)
                .child(section)
                .into_any_element()
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .border_b(RULE)
        .border_color(palette.line)
        .child(
            div()
                .px(SPACE_8)
                .pt(SPACE_7)
                .pb(SPACE_4)
                .child(SectionLabel::new("Workloads")),
        )
        .child(
            div()
                .border_t(RULE)
                .border_color(palette.line)
                .children(rows),
        )
        .into_any_element()
}
