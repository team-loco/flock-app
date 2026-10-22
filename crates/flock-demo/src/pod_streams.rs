use std::thread;
use std::time::Duration;

use flock_core::pods::{Container, ContainerState, Pod};
use flock_core::{
    Command, ExecEvent, ExecRequest, LogLevel, LogLine, LogRequest, NodeShellRequest, Snapshot,
    Status,
};

use crate::workload::Workloads;

const TAIL_TICK: Duration = Duration::from_secs(2);
const TAIL_START_MS: u64 = (21 * 3600 + 14 * 60 + 4) * 1000 + 4;
const TAIL_STEP_MS: u64 = 2000;
const MS_PER_SECOND: u64 = 1000;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const PROMPT: &str = "/app $ ";
const NEWLINE: &str = "\r\n";
const NODE_SHELL_NAMESPACE: &str = "default";
const NODE_SHELL_PREFIX: &str = "node-shell-";
const ERASE: &[u8] = b"\x08 \x08";
const INTERRUPT: &str = "^C";
const ESC: u8 = 0x1b;
const CSI: u8 = b'[';
const DEL: u8 = 0x7f;
const BACKSPACE: u8 = 0x08;
const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const ENTER: u8 = b'\r';
const PRINTABLE_FROM: u8 = 0x20;
const EXIT_NOT_FOUND: i32 = 1;
const WAITING_TIME: &str = "21:14:03.000";
const WAITING_HINT: &str = "logs will stream here once the container starts";

const HEALTHY_LOGS: [(&str, LogLevel, &str); 9] = [
    ("21:13:58.004", LogLevel::Info, "GET /healthz 200 0.4ms"),
    (
        "21:13:58.731",
        LogLevel::Info,
        "POST /v1/cart/9312/items 201 18.2ms",
    ),
    (
        "21:13:59.112",
        LogLevel::Info,
        "GET /v1/cart/9312 200 6.9ms",
    ),
    ("21:14:00.004", LogLevel::Info, "GET /healthz 200 0.3ms"),
    (
        "21:14:00.540",
        LogLevel::Debug,
        "cache hit sku:44718 ttl=287s",
    ),
    (
        "21:14:01.276",
        LogLevel::Warn,
        "slow query 412ms: SELECT … FROM carts WHERE id = $1",
    ),
    ("21:14:02.004", LogLevel::Info, "GET /healthz 200 0.4ms"),
    (
        "21:14:02.861",
        LogLevel::Info,
        "POST /v1/checkout 200 96.1ms",
    ),
    (
        "21:14:03.402",
        LogLevel::Info,
        "GET /v1/cart/9318 200 5.2ms",
    ),
];

const TAIL: [&str; 4] = [
    "GET /healthz 200 0.4ms",
    "POST /v1/cart/9318/items 201 17.6ms",
    "GET /v1/cart/9318 200 5.8ms",
    "GET /healthz 200 0.3ms",
];

const HISTORY: [(&str, &str); 2] = [
    ("ls", "bin  config  migrations  server"),
    ("env | grep -c ^STRIPE", "2"),
];

pub fn handle(command: &Command, snapshot: &Snapshot, workloads: &Workloads) -> bool {
    match command {
        Command::StreamLogs(request) => {
            let Some(pod) = owned(snapshot, &request.pod, workloads) else {
                return false;
            };
            let request = request.clone();
            spawn("flock-demo-pod-logs", move || stream_logs(&pod, &request));
            true
        }
        Command::NodeShell(request) => {
            let request = request.clone();
            spawn("flock-demo-node-shell", move || run_node_shell(&request));
            true
        }
        Command::Exec(request) => {
            let Some(pod) = owned(snapshot, &request.target.pod, workloads) else {
                return false;
            };
            let request = request.clone();
            spawn("flock-demo-pod-exec", move || run_shell(&pod, &request));
            true
        }
        _ => false,
    }
}

fn owned(
    snapshot: &Snapshot,
    target: &flock_core::ResourceRef,
    workloads: &Workloads,
) -> Option<Pod> {
    if workloads.knows_pod(target) {
        return None;
    }
    snapshot.pods.iter().find(|pod| pod.is(target)).cloned()
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(error) = thread::Builder::new().name(name.into()).spawn(work) {
        tracing::error!(%error, "demo worker failed to start");
    }
}

fn container<'a>(pod: &'a Pod, name: &str) -> Option<&'a Container> {
    pod.containers.iter().find(|c| c.name == name)
}

pub fn initial_logs(pod: &Pod, request: &LogRequest) -> Vec<LogLine> {
    let Some(container) = container(pod, &request.container) else {
        return vec![LogLine::marker(format!(
            "── container {} not found in pod {} ──",
            request.container, pod.name
        ))];
    };
    if container.state == ContainerState::Waiting {
        let waiting = format!(
            "container {} is waiting to start: {}",
            container.name,
            container.note.as_deref().unwrap_or(&container.reason)
        );
        return vec![
            LogLine::new(WAITING_TIME, LogLevel::Warn, waiting),
            LogLine::new(WAITING_TIME, LogLevel::Debug, WAITING_HINT),
        ];
    }
    HEALTHY_LOGS
        .iter()
        .map(|(time, level, message)| LogLine::new(time, *level, *message))
        .collect()
}

pub fn tails(pod: &Pod, request: &LogRequest) -> bool {
    container(pod, &request.container)
        .is_some_and(|c| c.state == ContainerState::Running && c.status == Status::Ok)
}

pub fn tail_line(n: u64) -> LogLine {
    let at = TAIL_START_MS + n * TAIL_STEP_MS;
    let seconds = at / MS_PER_SECOND;
    let time = format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / SECONDS_PER_HOUR,
        seconds % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
        seconds % SECONDS_PER_MINUTE,
        at % MS_PER_SECOND
    );
    let ix = usize::try_from(n).unwrap_or_default() % TAIL.len();
    let message = TAIL.get(ix).copied().unwrap_or_default();
    LogLine::new(&time, LogLevel::Info, message)
}

fn stream_logs(pod: &Pod, request: &LogRequest) {
    for line in initial_logs(pod, request) {
        if !request.sink.send_blocking(line) {
            return;
        }
    }
    if !tails(pod, request) {
        return;
    }
    for n in 0.. {
        thread::sleep(TAIL_TICK);
        if !request.sink.send_blocking(tail_line(n)) {
            return;
        }
    }
}

pub fn banner(pod: &Pod, container: &str) -> Result<String, String> {
    let started = pod
        .containers
        .iter()
        .find(|c| c.name == container)
        .is_some_and(|c| c.state != ContainerState::Waiting);
    if !started {
        return Err(format!(
            "error: unable to upgrade connection: container not found (\"{container}\"){NEWLINE}"
        ));
    }
    let mut out = String::new();
    for (command, output) in HISTORY {
        out.push_str(PROMPT);
        out.push_str(command);
        out.push_str(NEWLINE);
        out.push_str(output);
        out.push_str(NEWLINE);
    }
    out.push_str(PROMPT);
    Ok(out)
}

pub fn reply(pod: &Pod, command: &str) -> Option<String> {
    let command = command.trim();
    let known = HISTORY.iter().find(|(line, _)| *line == command);
    let output = match command {
        "" => return Some(PROMPT.to_owned()),
        "exit" => return None,
        "pwd" => "/app".to_owned(),
        "whoami" => "app".to_owned(),
        "hostname" => pod.name.clone(),
        other => known.map_or_else(
            || {
                let program = other.split_whitespace().next().unwrap_or(other);
                format!("sh: {program}: not found")
            },
            |(_, output)| (*output).to_owned(),
        ),
    };
    Some(format!("{output}{NEWLINE}{PROMPT}"))
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    Started,
    Csi,
}

#[derive(Default)]
pub struct Line {
    text: String,
    escape: Escape,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Echo(Vec<u8>),
    Exit,
}

impl Line {
    pub fn feed(&mut self, pod: &Pod, bytes: &[u8]) -> Vec<Step> {
        let mut steps = Vec::new();
        for &byte in bytes {
            match (self.escape, byte) {
                (Escape::Started, CSI) => self.escape = Escape::Csi,
                (Escape::Started, _) => self.escape = Escape::None,
                (Escape::Csi, b'@'..=b'~') => self.escape = Escape::None,
                (Escape::Csi, _) => {}
                (Escape::None, ESC) => self.escape = Escape::Started,
                (Escape::None, ENTER) => {
                    let command = std::mem::take(&mut self.text);
                    let Some(output) = reply(pod, &command) else {
                        steps.push(Step::Echo(NEWLINE.as_bytes().to_vec()));
                        steps.push(Step::Exit);
                        return steps;
                    };
                    steps.push(Step::Echo(format!("{NEWLINE}{output}").into_bytes()));
                }
                (Escape::None, DEL | BACKSPACE) => {
                    if self.text.pop().is_some() {
                        steps.push(Step::Echo(ERASE.to_vec()));
                    }
                }
                (Escape::None, CTRL_C) => {
                    self.text.clear();
                    steps.push(Step::Echo(
                        format!("{INTERRUPT}{NEWLINE}{PROMPT}").into_bytes(),
                    ));
                }
                (Escape::None, CTRL_D) => {
                    steps.push(Step::Exit);
                    return steps;
                }
                (Escape::None, b) if b >= PRINTABLE_FROM => {
                    self.text.push(char::from(b));
                    steps.push(Step::Echo(vec![b]));
                }
                (Escape::None, _) => {}
            }
        }
        steps
    }
}

fn node_prompt(node: &str) -> String {
    format!("root@{node}:/# ")
}

fn node_echo(bytes: &[u8], prompt: &str) -> Vec<u8> {
    let newline = format!("{NEWLINE}{prompt}");
    bytes
        .iter()
        .flat_map(|byte| match byte {
            &ENTER => newline.clone().into_bytes(),
            other => vec![*other],
        })
        .collect()
}

fn run_node_shell(request: &NodeShellRequest) {
    let name = &request.node.name;
    let started = format!("Pod: {NODE_SHELL_NAMESPACE}/{NODE_SHELL_PREFIX}{name}");
    let prompt = node_prompt(name);
    if !request
        .output
        .send_blocking(ExecEvent::Started { note: started })
    {
        return;
    }
    if !request
        .output
        .send_blocking(ExecEvent::Output(prompt.clone().into_bytes()))
    {
        return;
    }
    while let Some(bytes) = request.input.recv_blocking() {
        let echo = node_echo(&bytes, &prompt);
        if !request.output.send_blocking(ExecEvent::Output(echo)) {
            return;
        }
    }
}

fn run_shell(pod: &Pod, request: &ExecRequest) {
    let banner = match banner(pod, &request.target.container) {
        Ok(banner) => banner,
        Err(error) => {
            request
                .output
                .send_blocking(ExecEvent::Output(error.into_bytes()));
            request.output.send_blocking(ExecEvent::Exited {
                code: Some(EXIT_NOT_FOUND),
            });
            return;
        }
    };
    if !request
        .output
        .send_blocking(ExecEvent::Output(banner.into_bytes()))
    {
        return;
    }
    let mut line = Line::default();
    while let Some(bytes) = request.input.recv_blocking() {
        for step in line.feed(pod, &bytes) {
            let event = match step {
                Step::Echo(bytes) => ExecEvent::Output(bytes),
                Step::Exit => {
                    request
                        .output
                        .send_blocking(ExecEvent::Exited { code: Some(0) });
                    return;
                }
            };
            if !request.output.send_blocking(event) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use flock_core::{ExecTarget, Sink, Source};

    use super::*;

    #[test]
    fn node_shells_echo_input_and_prompt_after_enter() {
        let prompt = node_prompt("n1");
        assert_eq!(prompt, "root@n1:/# ");
        assert_eq!(node_echo(b"ls\r", &prompt), b"ls\r\nroot@n1:/# ".to_vec());
    }
    use crate::pods::pods;

    fn sample(name: &str) -> Pod {
        pods()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap_or_default()
    }

    fn logs(pod: &Pod, container: &str) -> LogRequest {
        let (sink, _rx) = Sink::channel();
        LogRequest {
            pod: pod.resource(),
            container: container.into(),
            instance: flock_core::LogInstance::Current,
            sink,
        }
    }

    #[test]
    fn running_containers_show_the_design_lines_and_tail() {
        let pod = sample("payments-db-0");
        let request = logs(&pod, "postgres");
        let lines = initial_logs(&pod, &request);
        assert_eq!(lines.len(), HEALTHY_LOGS.len());
        assert_eq!(
            lines.first(),
            Some(&LogLine::new(
                "21:13:58.004",
                LogLevel::Info,
                "GET /healthz 200 0.4ms"
            ))
        );
        assert!(tails(&pod, &request));
        assert_eq!(tail_line(0).time.as_deref(), Some("21:14:04.004"));
        assert_eq!(tail_line(30).time.as_deref(), Some("21:15:04.004"));
    }

    #[test]
    fn waiting_containers_explain_why_there_are_no_logs() {
        let pod = sample("ml-batch-5c7f-9kq2d");
        let request = logs(&pod, "trainer");
        let lines = initial_logs(&pod, &request);
        assert_eq!(
            lines.first().map(|l| l.message.as_str()),
            Some("container trainer is waiting to start: unschedulable · nvidia.com/gpu: 1")
        );
        assert!(!tails(&pod, &request));
    }

    #[test]
    fn shell_replays_the_design_transcript_and_answers() {
        let pod = sample("payments-db-0");
        let banner = banner(&pod, "postgres").unwrap_or_default();
        assert!(banner.ends_with("env | grep -c ^STRIPE\r\n2\r\n/app $ "));
        let mut line = Line::default();
        let steps = line.feed(&pod, b"lx\x7fs\r");
        assert_eq!(steps.get(2), Some(&Step::Echo(ERASE.to_vec())));
        assert_eq!(
            steps.last(),
            Some(&Step::Echo(
                b"\r\nbin  config  migrations  server\r\n/app $ ".to_vec()
            ))
        );
        let unknown = line.feed(&pod, b"\x1b[Acurl x\r");
        let Some(Step::Echo(last)) = unknown.last() else {
            panic!("expected an echo");
        };
        assert!(String::from_utf8_lossy(last).contains("sh: curl: not found"));
        assert_eq!(line.feed(&pod, b"exit\r").last(), Some(&Step::Exit));
    }

    #[test]
    fn unstarted_containers_refuse_exec() {
        let pod = sample("ml-batch-5c7f-9kq2d");
        assert!(banner(&pod, "trainer").is_err());
    }

    #[test]
    fn workload_pods_are_left_to_the_workload_simulation() {
        let snapshot = crate::snapshot();
        let workloads = Workloads::new(&snapshot);
        let checkout = sample("checkout-api-7f9c6d-x2lqp");
        let db = sample("payments-db-0");
        let (input, source) = Source::channel();
        let (output, _events) = Sink::channel();
        let exec = |pod: &Pod| {
            Command::Exec(ExecRequest {
                target: ExecTarget {
                    pod: pod.resource(),
                    container: pod
                        .containers
                        .first()
                        .map(|c| c.name.clone())
                        .unwrap_or_default(),
                    debug_image: None,
                },
                input: source.clone(),
                resize: Source::channel().1,
                output: output.clone(),
            })
        };
        assert!(!handle(&exec(&checkout), &snapshot, &workloads));
        assert!(handle(&exec(&db), &snapshot, &workloads));
        drop(input);
    }
}
