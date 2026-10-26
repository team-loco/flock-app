use std::future::Future;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use async_channel::Receiver;
use gpui::{AsyncApp, Context, WeakEntity};

pub const STREAM_BATCH: usize = 16;

#[derive(Default)]
struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
        if self.yielded {
            return Poll::Ready(());
        }
        self.yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

pub async fn drain<V: 'static, T>(
    events: Receiver<T>,
    this: WeakEntity<V>,
    cx: &mut AsyncApp,
    mut apply: impl FnMut(&mut V, Vec<T>, &mut Context<V>),
) {
    while let Ok(first) = events.recv().await {
        let mut batch = Vec::with_capacity(STREAM_BATCH);
        batch.push(first);
        while batch.len() < STREAM_BATCH {
            let Ok(next) = events.try_recv() else {
                break;
            };
            batch.push(next);
        }
        if this.update(cx, |view, cx| apply(view, batch, cx)).is_err() {
            break;
        }
        YieldNow::default().await;
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    const BACKLOG: usize = STREAM_BATCH * 4 + 3;

    #[derive(Default)]
    struct Collected {
        items: Vec<usize>,
        batches: Vec<usize>,
    }

    #[gpui::test]
    fn a_backlog_is_applied_fully_in_order_across_bounded_batches(cx: &mut TestAppContext) {
        let (sender, events) = async_channel::unbounded();
        for item in 0..BACKLOG {
            assert!(sender.try_send(item).is_ok());
        }
        drop(sender);
        let collected = cx.new(|_| Collected::default());
        let weak = collected.downgrade();
        cx.spawn(async move |mut cx| {
            drain(events, weak, &mut cx, |view: &mut Collected, batch, _| {
                view.batches.push(batch.len());
                view.items.extend(batch);
            })
            .await;
        })
        .detach();
        cx.run_until_parked();
        collected.read_with(cx, |view, _| {
            assert_eq!(view.items, (0..BACKLOG).collect::<Vec<_>>());
            assert!(view.batches.len() > 1);
            assert!(view.batches.iter().all(|len| *len <= STREAM_BATCH));
        });
    }
}
