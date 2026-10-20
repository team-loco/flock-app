use flock_core::{LogInstance, LogLevel, LogLine, Status};

use super::data::API;

const CLOCK_START_MS: u64 = (21 * 3600 + 14 * 60 + 3) * 1000 + 402;
const MS_PER_SECOND: u64 = 1000;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
const TICK_STEPS_MS: [u64; 4] = [412, 731, 268, 589];

const REQUESTS: [(LogLevel, &str); 6] = [
    (LogLevel::Info, "GET /healthz 200 0.3ms"),
    (LogLevel::Info, "POST /v1/cart/9324/items 201 17.4ms"),
    (LogLevel::Info, "GET /v1/cart/9324 200 6.1ms"),
    (LogLevel::Info, "POST /v1/checkout 200 88.7ms"),
    (
        LogLevel::Warn,
        "slow query 388ms: SELECT … FROM carts WHERE id = $1",
    ),
    (LogLevel::Info, "GET /v1/cart/9331 200 5.8ms"),
];

const ENVOY_REQUESTS: [(LogLevel, &str); 3] = [
    (LogLevel::Info, "upstream checkout-api:8080 healthy"),
    (LogLevel::Debug, "cds: 4 clusters, 0 warming"),
    (LogLevel::Info, "listener https_8443: 3 active connections"),
];

pub fn crash_waiting() -> Vec<LogLine> {
    vec![LogLine::marker(
        "── container api is waiting to start: CrashLoopBackOff (back-off 5m0s) ──",
    )]
}

pub fn crashed_api() -> Vec<LogLine> {
    vec![
        LogLine::new(
            "14:08:51.118",
            LogLevel::Info,
            r#"{"level":"info","ts":"2026-10-20T14:08:51.118Z","msg":"GET /api/info","status":200,"duration":"1.2ms"}"#,
        ),
        LogLine::new(
            "14:09:04.902",
            LogLevel::Error,
            r#"{"level":"error","ts":"2026-10-20T14:09:04.902Z","msg":"failed to load config","path":"/data/config.yaml","err":"open /data/config.yaml: no such file or directory"}"#,
        ),
        LogLine::new(
            "14:09:04.903",
            LogLevel::Fatal,
            r#"{"level":"fatal","ts":"2026-10-20T14:09:04.903Z","msg":"exiting","code":1,"caller":"checkout/main.go:131"}"#,
        ),
    ]
}

pub fn healthy_api() -> Vec<LogLine> {
    vec![
        LogLine::new("21:14:00.004", LogLevel::Info, "GET /healthz 200 0.3ms"),
        LogLine::new(
            "21:14:00.731",
            LogLevel::Info,
            "POST /v1/cart/9312/items 201 18.2ms",
        ),
        LogLine::new(
            "21:14:01.112",
            LogLevel::Info,
            "GET /v1/cart/9312 200 6.9ms",
        ),
        LogLine::new(
            "21:14:01.276",
            LogLevel::Warn,
            "slow query 412ms: SELECT … FROM carts WHERE id = $1",
        ),
        LogLine::new("21:14:02.004", LogLevel::Info, "GET /healthz 200 0.4ms"),
        LogLine::new(
            "21:14:02.861",
            LogLevel::Info,
            "POST /v1/checkout 200 96.1ms",
        ),
        LogLine::new(
            "21:14:03.402",
            LogLevel::Info,
            "GET /v1/cart/9318 200 5.2ms",
        ),
    ]
}

pub struct Script {
    pub initial: Vec<LogLine>,
    pub streams: bool,
}

pub fn script(pod_status: Status, container: &str, instance: LogInstance) -> Script {
    let api = container == API;
    if instance == LogInstance::Previous {
        let initial = if api { crashed_api() } else { Vec::new() };
        return Script {
            initial,
            streams: false,
        };
    }
    let crashing = pod_status == Status::Err;
    let initial = if api && crashing {
        crash_waiting()
    } else if api {
        healthy_api()
    } else {
        Vec::new()
    };
    let streams = !(api && crashing);
    Script { initial, streams }
}

pub struct Ticker {
    clock_ms: u64,
    step: usize,
    api: bool,
}

impl Ticker {
    pub fn new(container: &str) -> Self {
        Self {
            clock_ms: CLOCK_START_MS,
            step: 0,
            api: container == API,
        }
    }

    pub fn next_line(&mut self) -> LogLine {
        let delay = TICK_STEPS_MS
            .get(self.step % TICK_STEPS_MS.len())
            .copied()
            .unwrap_or_default();
        self.clock_ms += delay;
        let (level, message) = if self.api {
            REQUESTS.get(self.step % REQUESTS.len()).copied()
        } else {
            ENVOY_REQUESTS
                .get(self.step % ENVOY_REQUESTS.len())
                .copied()
        }
        .unwrap_or((LogLevel::Info, ""));
        self.step += 1;
        LogLine::new(&clock(self.clock_ms), level, message)
    }
}

pub fn clock(ms: u64) -> String {
    let millis = ms % MS_PER_SECOND;
    let seconds = ms / MS_PER_SECOND % SECONDS_PER_MINUTE;
    let minutes = ms / MS_PER_SECOND / SECONDS_PER_MINUTE % MINUTES_PER_HOUR;
    let hours = ms / MS_PER_SECOND / SECONDS_PER_MINUTE / MINUTES_PER_HOUR % HOURS_PER_DAY;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_like_the_design() {
        assert_eq!(clock(CLOCK_START_MS), "21:14:03.402");
        assert_eq!(clock(0), "00:00:00.000");
    }

    #[test]
    fn ticker_moves_time_forward() {
        let mut ticker = Ticker::new(API);
        let first = ticker.next_line();
        let second = ticker.next_line();
        assert_eq!(first.time.as_deref(), Some("21:14:03.814"));
        assert!(second.time > first.time);
        assert_eq!(first.message, "GET /healthz 200 0.3ms");
    }

    #[test]
    fn crashing_api_explains_waiting_and_does_not_stream() {
        let current = script(Status::Err, API, LogInstance::Current);
        assert!(!current.streams);
        assert_eq!(current.initial, crash_waiting());
        let previous = script(Status::Err, API, LogInstance::Previous);
        assert!(!previous.streams);
        assert_eq!(previous.initial, crashed_api());
    }

    #[test]
    fn healthy_containers_stream() {
        let api = script(Status::Ok, API, LogInstance::Current);
        assert!(api.streams);
        assert_eq!(api.initial, healthy_api());
        let envoy = script(Status::Err, "envoy", LogInstance::Current);
        assert!(envoy.streams);
    }
}
