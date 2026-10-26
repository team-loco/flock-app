use std::sync::Arc;
use std::time::Instant;

use flock_core::notify::{Alert, Notifier};
use flock_core::{Command, Receiver, Sender, Snapshot, Update};
use gpui::{App, Context, SystemNotification, Task};

use crate::kit::{Toast, toast};

#[derive(Default)]
struct Updates {
    snapshot: Option<Arc<Snapshot>>,
    subscribers: Vec<Sender<Update>>,
}

impl Updates {
    fn subscribe(&mut self) -> Receiver<Update> {
        let (sender, receiver) = async_channel::unbounded();
        if let Some(snapshot) = &self.snapshot {
            let update = Update::Snapshot(snapshot.clone());
            let _ = sender.try_send(update);
        }
        self.subscribers.push(sender);
        receiver
    }

    fn publish(&mut self, update: &Update) {
        if let Update::Snapshot(snapshot) = update {
            self.snapshot = Some(snapshot.clone());
        }
        self.subscribers
            .retain(|sender| sender.try_send(update.clone()).is_ok());
    }
}

pub struct Store {
    updates: Updates,
    notifier: Notifier,
    commands: Sender<Command>,
    _listen: Task<()>,
}

impl Store {
    pub fn new(
        updates: Receiver<Update>,
        commands: Sender<Command>,
        cx: &mut Context<Self>,
    ) -> Self {
        let listen = cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                if this
                    .update(cx, |store, cx| {
                        if let Update::Notice(notice) = &update {
                            let shown = Toast::new(notice.status, notice.title.clone());
                            let shown = match &notice.detail {
                                Some(detail) => shown.detail(detail.clone()),
                                None => shown,
                            };
                            toast(shown, cx);
                        }
                        if let Update::Snapshot(snapshot) = &update {
                            store.notify(snapshot, cx);
                        }
                        store.updates.publish(&update);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            updates: Updates::default(),
            notifier: Notifier::default(),
            commands,
            _listen: listen,
        }
    }

    fn notify(&mut self, snapshot: &Snapshot, cx: &mut App) {
        let settings = crate::settings::current(cx).notifications;
        let alerts = self.notifier.observe(snapshot, settings, Instant::now());
        for alert in alerts {
            cx.show_system_notification(system_notification(alert));
        }
    }

    pub fn subscribe(&mut self) -> Receiver<Update> {
        self.updates.subscribe()
    }

    pub fn commands(&self) -> Sender<Command> {
        self.commands.clone()
    }
}

fn system_notification(alert: Alert) -> SystemNotification {
    SystemNotification {
        tag: alert.tag.into(),
        title: alert.title.into(),
        body: alert.body.into(),
        actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_share_one_snapshot_and_late_windows_receive_the_latest() {
        let mut updates = Updates::default();
        let first = updates.subscribe();
        let second = updates.subscribe();
        let shared: Arc<Snapshot> = Arc::default();
        let snapshot = Update::Snapshot(Arc::clone(&shared));
        let receives_shared = |receiver: &Receiver<Update>| {
            let received = receiver.try_recv();
            matches!(&received, Ok(Update::Snapshot(got)) if Arc::ptr_eq(got, &shared))
        };
        updates.publish(&snapshot);
        assert!(receives_shared(&first));
        assert!(receives_shared(&second));
        drop(first);
        let third = updates.subscribe();
        assert!(receives_shared(&third));
        updates.publish(&snapshot);
        assert!(receives_shared(&second));
        assert!(receives_shared(&third));
        assert_eq!(updates.subscribers.len(), 2);
    }
}
