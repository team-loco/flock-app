use std::fmt;

use async_channel::{Receiver, Sender};

use crate::ResourceRef;

pub struct Sink<T>(Sender<T>);

impl<T> Sink<T> {
    pub fn channel() -> (Self, Receiver<T>) {
        let (tx, rx) = async_channel::unbounded();
        (Self(tx), rx)
    }

    pub fn send_blocking(&self, value: T) -> bool {
        self.0.send_blocking(value).is_ok()
    }

    pub async fn closed(&self) {
        self.0.closed().await;
    }

    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

impl<T> Clone for Sink<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> fmt::Debug for Sink<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sink")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl<T> PartialEq for Sink<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }
}

impl<T> Eq for Sink<T> {}

pub struct Source<T>(Receiver<T>);

impl<T> Source<T> {
    pub fn channel() -> (Sender<T>, Self) {
        let (tx, rx) = async_channel::unbounded();
        (tx, Self(rx))
    }

    pub async fn recv(&self) -> Option<T> {
        self.0.recv().await.ok()
    }

    pub fn recv_blocking(&self) -> Option<T> {
        self.0.recv_blocking().ok()
    }
}

impl<T> Clone for Source<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> fmt::Debug for Source<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source")
            .field("closed", &self.0.is_closed())
            .finish()
    }
}

impl<T> PartialEq for Source<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_channel(&other.0)
    }
}

impl<T> Eq for Source<T> {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn from_word(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "trace" => Some(Self::Debug),
            "debug" => Some(Self::Debug),
            "dbg" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "inf" => Some(Self::Info),
            "notice" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "warning" => Some(Self::Warn),
            "wrn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "err" => Some(Self::Error),
            "fatal" => Some(Self::Fatal),
            "panic" => Some(Self::Fatal),
            "critical" => Some(Self::Fatal),
            "crit" => Some(Self::Fatal),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogInstance {
    #[default]
    Current,
    Previous,
}

impl LogInstance {
    pub const ALL: [Self; 2] = [Self::Current, Self::Previous];

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Previous => "previous",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub time: Option<String>,
    pub level: Option<LogLevel>,
    pub message: String,
}

impl LogLine {
    pub fn new(time: &str, level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            time: Some(time.into()),
            level: Some(level),
            message: message.into(),
        }
    }

    pub fn marker(message: impl Into<String>) -> Self {
        Self {
            time: None,
            level: None,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRequest {
    pub pod: ResourceRef,
    pub container: String,
    pub instance: LogInstance,
    pub sink: Sink<LogLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecTarget {
    pub pod: ResourceRef,
    pub container: String,
    pub debug_image: Option<String>,
}

impl ExecTarget {
    pub fn is_debug(&self) -> bool {
        self.debug_image.is_some()
    }

    pub fn command_line(&self) -> String {
        match &self.debug_image {
            Some(image) => format!(
                "$ kubectl debug -it {} --image={image} --target={}",
                self.pod.name, self.container
            ),
            None => format!(
                "$ kubectl exec -it {} -c {} -- sh",
                self.pod.name, self.container
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecEvent {
    Started { note: String },
    Output(Vec<u8>),
    NoShell,
    Exited { code: Option<i32> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRequest {
    pub target: ExecTarget,
    pub input: Source<Vec<u8>>,
    pub resize: Source<TermSize>,
    pub output: Sink<ExecEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinks_compare_by_channel() {
        let (a, _rx_a) = Sink::<u8>::channel();
        let (b, _rx_b) = Sink::<u8>::channel();
        assert_eq!(a, a.clone());
        assert_ne!(a, b);
    }

    #[test]
    fn sink_reports_a_dropped_receiver() {
        let (sink, rx) = Sink::channel();
        assert!(sink.send_blocking(1));
        drop(rx);
        assert!(sink.is_closed());
        assert!(!sink.send_blocking(2));
    }

    #[test]
    fn exec_target_prints_the_equivalent_kubectl() {
        let mut target = ExecTarget {
            pod: ResourceRef {
                cluster: crate::ClusterId::new("prod-us-east"),
                kind: "Pod".into(),
                namespace: Some("payments".into()),
                name: "checkout-api-7f9c6d-x2lqp".into(),
            },
            container: "api".into(),
            debug_image: Some("busybox:1.36".into()),
        };
        assert_eq!(
            target.command_line(),
            "$ kubectl debug -it checkout-api-7f9c6d-x2lqp --image=busybox:1.36 --target=api"
        );
        target.debug_image = None;
        assert_eq!(
            target.command_line(),
            "$ kubectl exec -it checkout-api-7f9c6d-x2lqp -c api -- sh"
        );
    }

    #[test]
    fn level_words_ignore_case() {
        assert_eq!(LogLevel::from_word("WARNING"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_word("crit"), Some(LogLevel::Fatal));
        assert_eq!(LogLevel::from_word("chatty"), None);
    }

    #[test]
    fn source_ends_when_every_sender_is_gone() {
        let (tx, source) = Source::channel();
        assert!(tx.send_blocking(7).is_ok());
        drop(tx);
        assert_eq!(source.recv_blocking(), Some(7));
        assert_eq!(source.recv_blocking(), None);
    }
}
