use std::time::{Duration, SystemTime};

use flock_core::app_settings::Updates;
use flock_core::time::age_since;
use flock_core::{Receiver, Sender, Status, Trigger, UpdaterRequest, UpdaterState, UpdaterStatus};
use gpui::{
    AnyElement, App, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, div,
};

use crate::actions::{CheckForUpdates, RestartToUpdate};
use crate::kit::{Button, Link, Toast, toast};
use crate::settings::{self, Settings};
use crate::theme::ActiveTheme as _;

const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const STARTUP_DELAY: Duration = Duration::from_secs(20);
pub const STATUS_ITEM: &str = "status-update";
const UP_TO_DATE: &str = "Flock is up to date";
const RETRY: &str = "Retry";
const FAILED: &str = "Could not update Flock";
const RESTART: &str = "Restart to update";
const CHECKING: &str = "Checking for updates…";
const NEVER_CHECKED: &str = "Not checked yet";

pub struct Updater {
    status: UpdaterStatus,
    requests: Option<Sender<UpdaterRequest>>,
    scheduled: Updates,
    timer: Option<Task<()>>,
    _listen: Option<Task<()>>,
    _settings: Option<Subscription>,
}

impl Global for Updater {}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(Updater {
        status: UpdaterStatus::default(),
        requests: None,
        scheduled: Updates::default(),
        timer: None,
        _listen: None,
        _settings: None,
    });
    cx.on_action(|_: &CheckForUpdates, cx| check_now(cx));
    cx.on_action(|_: &RestartToUpdate, cx| restart(cx));
}

pub fn install(requests: Sender<UpdaterRequest>, statuses: Receiver<UpdaterStatus>, cx: &mut App) {
    let listen = cx.spawn(async move |cx| {
        while let Ok(status) = statuses.recv().await {
            cx.update(|cx| receive(status, cx));
        }
    });
    let observe = cx.observe_global::<Settings>(reschedule);
    let scheduled = settings::current(cx).updates;
    let schedule = schedule(STARTUP_DELAY, cx);
    cx.set_global(Updater {
        status: UpdaterStatus::default(),
        requests: Some(requests),
        scheduled,
        timer: Some(schedule),
        _listen: Some(listen),
        _settings: Some(observe),
    });
}

fn schedule(first: Duration, cx: &App) -> Task<()> {
    cx.spawn(async move |cx| {
        let mut delay = first;
        loop {
            cx.background_executor().timer(delay).await;
            delay = cx.update(|cx| {
                let updates = settings::current(cx).updates;
                if updates.automatic {
                    send(check(Trigger::Automatic, cx), cx);
                }
                updates.interval.duration()
            });
        }
    })
}

fn reschedule(cx: &mut App) {
    let wanted = settings::current(cx).updates;
    let current = cx.global::<Updater>();
    if current.requests.is_none() || current.scheduled == wanted {
        return;
    }
    let task = schedule(wanted.interval.duration(), cx);
    let updater = cx.global_mut::<Updater>();
    updater.scheduled = wanted;
    updater.timer = Some(task);
}

fn check(trigger: Trigger, cx: &App) -> UpdaterRequest {
    UpdaterRequest::Check {
        trigger,
        prereleases: settings::current(cx).updates.prereleases,
    }
}

fn send(request: UpdaterRequest, cx: &App) {
    let Some(requests) = &cx.global::<Updater>().requests else {
        return;
    };
    if let Err(error) = requests.try_send(request) {
        tracing::warn!(%error, "the updater stopped accepting requests");
    }
}

pub fn check_now(cx: &mut App) {
    send(check(Trigger::Manual, cx), cx);
}

pub fn restart(cx: &mut App) {
    send(UpdaterRequest::Install, cx);
}

pub fn status(cx: &App) -> &UpdaterStatus {
    &cx.global::<Updater>().status
}

pub fn version() -> &'static str {
    VERSION
}

pub fn last_checked(cx: &App) -> String {
    status(cx).checked.map_or_else(
        || NEVER_CHECKED.to_owned(),
        |checked| format!("Last checked {} ago", age_since(checked, SystemTime::now())),
    )
}

fn receive(status: UpdaterStatus, cx: &mut App) {
    let notice = (status.trigger == Trigger::Manual)
        .then(|| notice(&status.state, VERSION))
        .flatten();
    let relaunching = status.state == UpdaterState::Relaunching;
    cx.global_mut::<Updater>().status = status;
    if let Some(notice) = notice {
        toast(notice, cx);
    }
    if relaunching {
        cx.quit();
    }
}

fn notice(state: &UpdaterState, current: &str) -> Option<Toast> {
    match state {
        UpdaterState::UpToDate => Some(
            Toast::new(Status::Ok, UP_TO_DATE)
                .detail(format!("Flock {current} is the newest release.")),
        ),
        UpdaterState::Failed { reason } => {
            let failed = Toast::new(Status::Err, FAILED)
                .detail(reason.clone())
                .action(RETRY, |_, cx| check_now(cx));
            Some(failed)
        }
        UpdaterState::Ready { version } => Some(
            Toast::new(Status::Ok, format!("Flock {version} is ready"))
                .detail("Restart Flock to update."),
        ),
        UpdaterState::Available { version, .. } => Some(
            Toast::new(Status::Ok, format!("Flock {version} is available"))
                .detail("Download the new version from its release page."),
        ),
        UpdaterState::Idle => None,
        UpdaterState::Checking => None,
        UpdaterState::Downloading { .. } => None,
        UpdaterState::Installing { .. } => None,
        UpdaterState::Relaunching => None,
    }
}

pub fn summary(state: &UpdaterState) -> Option<String> {
    match state {
        UpdaterState::Checking => Some(CHECKING.to_owned()),
        UpdaterState::Downloading { version } => Some(format!("Downloading Flock {version}…")),
        UpdaterState::Installing { version } => Some(format!("Installing Flock {version}…")),
        UpdaterState::Ready { version } => Some(format!("Flock {version} is ready to install")),
        UpdaterState::Available { version, .. } => Some(format!("Flock {version} is available")),
        UpdaterState::Failed { reason } => Some(format!("Update failed: {reason}")),
        UpdaterState::UpToDate => Some(UP_TO_DATE.to_owned()),
        UpdaterState::Idle => None,
        UpdaterState::Relaunching => None,
    }
}

pub(crate) fn status_item(cx: &App) -> Option<AnyElement> {
    let palette = *cx.palette();
    let state = &status(cx).state;
    match state {
        UpdaterState::Ready { .. } => {
            let button = Button::new(STATUS_ITEM, RESTART)
                .small()
                .primary()
                .on_click(|_, _, cx| restart(cx));
            Some(button.into_any_element())
        }
        UpdaterState::Available { version, page } => {
            let page = page.clone();
            let link = Link::new(STATUS_ITEM, format!("Download Flock {version}"))
                .on_click(move |_, _, cx| cx.open_url(&page));
            Some(link.into_any_element())
        }
        UpdaterState::Failed { .. } => {
            let text = summary(state)?;
            let item = div()
                .id(STATUS_ITEM)
                .debug_selector(|| STATUS_ITEM.to_owned())
                .cursor_pointer()
                .text_color(palette.err)
                .on_click(|_, _, cx| check_now(cx))
                .child(text);
            Some(item.into_any_element())
        }
        UpdaterState::Checking => text_item(state),
        UpdaterState::Downloading { .. } => text_item(state),
        UpdaterState::Installing { .. } => text_item(state),
        UpdaterState::Idle => None,
        UpdaterState::UpToDate => None,
        UpdaterState::Relaunching => None,
    }
}

fn text_item(state: &UpdaterState) -> Option<AnyElement> {
    let text = summary(state)?;
    let item = div().debug_selector(|| STATUS_ITEM.to_owned()).child(text);
    Some(item.into_any_element())
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use flock_core::AppSettings;
    use flock_core::app_settings::CheckInterval;
    use gpui::{SharedString, TestAppContext};

    use super::*;

    fn setup(cx: &mut TestAppContext) -> (Receiver<UpdaterRequest>, Sender<UpdaterStatus>) {
        let (requests, received) = async_channel::unbounded();
        let (statuses, status_rx) = async_channel::unbounded();
        cx.update(|cx| {
            crate::init(None, cx).unwrap_or_else(|error| panic!("UI init: {error}"));
            let save: settings::Saver = Rc::new(|_: &AppSettings| Ok(()));
            settings::install(AppSettings::default(), save, true, cx);
            install(requests, status_rx, cx);
        });
        (received, statuses)
    }

    fn automatic() -> UpdaterRequest {
        UpdaterRequest::Check {
            trigger: Trigger::Automatic,
            prereleases: false,
        }
    }

    #[gpui::test]
    fn the_first_check_waits_for_the_startup_delay_then_repeats_each_interval(
        cx: &mut TestAppContext,
    ) {
        let (received, _statuses) = setup(cx);
        cx.run_until_parked();
        assert!(received.is_empty());
        cx.executor().advance_clock(STARTUP_DELAY);
        assert_eq!(received.try_recv(), Ok(automatic()));
        cx.executor()
            .advance_clock(Updates::default().interval.duration());
        assert_eq!(received.try_recv(), Ok(automatic()));
        assert!(received.is_empty());
    }

    #[gpui::test]
    fn a_new_interval_applies_at_once_and_turning_checks_off_stops_them(cx: &mut TestAppContext) {
        let (received, _statuses) = setup(cx);
        cx.executor().advance_clock(STARTUP_DELAY);
        assert_eq!(received.try_recv(), Ok(automatic()));
        let hourly =
            cx.update(|cx| settings::update(cx, |s| s.updates.interval = CheckInterval::Hours1));
        assert_eq!(hourly, Ok(()));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(CheckInterval::Hours1.duration());
        assert_eq!(received.try_recv(), Ok(automatic()));
        let off = cx.update(|cx| settings::update(cx, |s| s.updates.automatic = false));
        assert_eq!(off, Ok(()));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(CheckInterval::Hours24.duration());
        assert_eq!(received.try_recv().ok(), None);
    }

    #[gpui::test]
    fn manual_results_raise_a_notice_and_automatic_ones_do_not(cx: &mut TestAppContext) {
        let (received, statuses) = setup(cx);
        cx.update(check_now);
        let manual = UpdaterRequest::Check {
            trigger: Trigger::Manual,
            prereleases: false,
        };
        assert_eq!(received.try_recv(), Ok(manual));
        let quiet = UpdaterStatus {
            state: UpdaterState::UpToDate,
            trigger: Trigger::Automatic,
            checked: Some(SystemTime::now()),
        };
        assert!(statuses.try_send(quiet).is_ok());
        cx.run_until_parked();
        assert_eq!(
            cx.read(crate::kit::toast_titles),
            Vec::<SharedString>::new()
        );
        let asked = UpdaterStatus {
            state: UpdaterState::UpToDate,
            trigger: Trigger::Manual,
            checked: Some(SystemTime::now()),
        };
        assert!(statuses.try_send(asked).is_ok());
        cx.run_until_parked();
        assert_eq!(cx.read(crate::kit::toast_titles), [UP_TO_DATE]);
        assert_eq!(
            cx.read(|cx| status(cx).state.clone()),
            UpdaterState::UpToDate
        );
        assert!(cx.read(last_checked).starts_with("Last checked"));
    }

    #[test]
    fn every_visible_state_has_a_one_line_summary() {
        let failed = UpdaterState::Failed {
            reason: "the signature is invalid".into(),
        };
        assert_eq!(
            summary(&failed).as_deref(),
            Some("Update failed: the signature is invalid")
        );
        let downloading = UpdaterState::Downloading {
            version: "0.2.0".into(),
        };
        assert_eq!(
            summary(&downloading).as_deref(),
            Some("Downloading Flock 0.2.0…")
        );
        assert_eq!(summary(&UpdaterState::Idle), None);
    }
}
