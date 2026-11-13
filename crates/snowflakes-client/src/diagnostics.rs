//! Opt-in, bounded local latency traces. Record command keys, never values.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

const MAX_PENDING: usize = 128;
const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub id: u64,
    key: u8,
    started: Instant,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Input,
    Dispatch,
    Received,
    Applied,
    Observed,
    Submitted,
    Superseded,
    Rejected,
    Evicted,
}
impl Stage {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Submitted | Self::Superseded | Self::Rejected | Self::Evicted
        )
    }
    fn bit(self) -> u16 {
        1 << self as u16
    }
}
#[derive(Serialize)]
struct Event {
    kind: &'static str,
    id: u64,
    command_key: u8,
    stage: Stage,
    elapsed_us: u64,
}
#[derive(Default)]
struct State {
    next: u64,
    pending: BTreeMap<u64, (Trace, u16)>,
}
#[derive(Default)]
struct Counters {
    dropped: AtomicU64,
    write_errors: AtomicU64,
    pending: AtomicU64,
}
pub struct Recorder {
    state: Mutex<State>,
    sender: Option<SyncSender<Event>>,
    counters: Arc<Counters>,
    worker: Option<JoinHandle<()>>,
}
impl Recorder {
    pub fn open(path: &Path) -> io::Result<Arc<Self>> {
        Self::open_with_limit(path, MAX_BYTES)
    }
    fn open_with_limit(path: &Path, max_bytes: usize) -> io::Result<Arc<Self>> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let (sender, receiver) = mpsc::sync_channel::<Event>(256);
        let counters = Arc::new(Counters::default());
        let stats = counters.clone();
        let worker = thread::spawn(move || {
            let mut writer = BufWriter::new(file);
            let header = serde_json::json!({"kind":"header", "schema_version":1,
                "version":env!("CARGO_PKG_VERSION"), "scope":"local_input_to_gpu_submission",
                "display_latency_measured":false});
            let mut bytes = 0;
            let mut write_line = |line: &str, limited: bool| -> io::Result<bool> {
                if limited && bytes + line.len() + 1 > max_bytes - 512 {
                    return Ok(false);
                }
                writer.write_all(line.as_bytes())?;
                writer.write_all(b"\n")?;
                bytes += line.len() + 1;
                Ok(true)
            };
            if write_line(&header.to_string(), false).is_err() {
                stats.write_errors.fetch_add(1, Ordering::Relaxed);
            }
            for event in receiver {
                match serde_json::to_string(&event)
                    .ok()
                    .map(|line| write_line(&line, true))
                {
                    Some(Ok(true)) => (),
                    Some(Ok(false)) => {
                        stats.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        stats.write_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            let summary = serde_json::json!({"kind":"summary", "dropped":stats.dropped.load(Ordering::Relaxed),
                "write_errors":stats.write_errors.load(Ordering::Relaxed), "unfinished":stats.pending.load(Ordering::Relaxed)});
            if write_line(&summary.to_string(), false).is_err() || writer.flush().is_err() {
                stats.write_errors.fetch_add(1, Ordering::Relaxed);
            }
        });
        Ok(Arc::new(Self {
            state: Mutex::new(State::default()),
            sender: Some(sender),
            counters,
            worker: Some(worker),
        }))
    }
    fn emit(&self, trace: Trace, stage: Stage) {
        let event = Event {
            kind: "latency",
            id: trace.id,
            command_key: trace.key,
            stage,
            elapsed_us: trace
                .started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64,
        };
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.try_send(event).is_err())
        {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn input(&self, key: u8) -> Trace {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.pending.len() == MAX_PENDING
            && let Some((_, (old, _))) = state.pending.pop_first()
        {
            self.emit(old, Stage::Evicted);
        }
        state.next = state.next.wrapping_add(1);
        let trace = Trace {
            id: state.next,
            key,
            started: Instant::now(),
        };
        state.pending.insert(trace.id, (trace, Stage::Input.bit()));
        self.emit(trace, Stage::Input);
        trace
    }
    pub fn record(&self, trace: Trace, stage: Stage) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some((_, bits)) = state.pending.get_mut(&trace.id) else {
            return;
        };
        if *bits & stage.bit() != 0 {
            return;
        }
        *bits |= stage.bit();
        self.emit(trace, stage);
        if stage.terminal() {
            state.pending.remove(&trace.id);
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.counters.pending.store(
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending
                .len() as u64,
            Ordering::Relaxed,
        );
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn events(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    #[test]
    fn trace_deduplicates_frames_and_preserves_terminal_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.ndjson");
        let recorder = Recorder::open(&path).unwrap();
        let submitted = recorder.input(1);
        for stage in [
            Stage::Dispatch,
            Stage::Received,
            Stage::Applied,
            Stage::Observed,
            Stage::Submitted,
        ] {
            recorder.record(submitted, stage);
            recorder.record(submitted, stage);
        }
        for stage in [Stage::Superseded, Stage::Rejected] {
            let trace = recorder.input(2);
            recorder.record(trace, stage);
            recorder.record(trace, Stage::Submitted);
        }
        recorder.input(3);
        drop(recorder);
        let events = events(&path);
        assert_eq!(events[0]["display_latency_measured"], false);
        let stages: Vec<_> = events
            .iter()
            .filter(|e| e["id"] == submitted.id)
            .map(|e| e["stage"].as_str().unwrap())
            .collect();
        assert_eq!(
            stages,
            [
                "input",
                "dispatch",
                "received",
                "applied",
                "observed",
                "submitted"
            ]
        );
        assert_eq!(
            events.iter().filter(|e| e["stage"] == "submitted").count(),
            1
        );
        let summary = events.last().unwrap();
        assert_eq!(summary["unfinished"], 1);
        assert_eq!(summary["dropped"], 0);
        assert_eq!(summary["write_errors"], 0);
        let mut previous = BTreeMap::new();
        for event in events.iter().filter(|e| e["kind"] == "latency") {
            let id = event["id"].as_u64().unwrap();
            let elapsed = event["elapsed_us"].as_u64().unwrap();
            assert!(elapsed >= previous.insert(id, elapsed).unwrap_or(0));
            let keys: Vec<_> = event
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, ["command_key", "elapsed_us", "id", "kind", "stage"]);
        }
    }
    #[test]
    fn trace_bounds_pending_work_and_file_size_and_reports_losses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bounded.ndjson");
        let recorder = Recorder::open_with_limit(&path, 4096).unwrap();
        let first = recorder.input(1);
        for _ in 0..5000 {
            recorder.input(1);
        }
        {
            let state = recorder.state.lock().unwrap();
            assert_eq!(state.pending.len(), MAX_PENDING);
            assert!(!state.pending.contains_key(&first.id));
        }
        recorder.record(first, Stage::Submitted);
        drop(recorder);
        assert!(std::fs::metadata(&path).unwrap().len() <= 4096);
        let events = events(&path);
        assert!(!events.iter().any(|e| e["stage"] == "submitted"));
        let summary = events.last().unwrap();
        assert_eq!(summary["kind"], "summary");
        assert_eq!(summary["unfinished"], MAX_PENDING);
        assert!(summary["dropped"].as_u64().unwrap() > 0);
        assert_eq!(summary["write_errors"], 0);
    }
    #[test]
    fn trace_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing");
        std::fs::write(&path, b"keep").unwrap();
        assert!(
            matches!(Recorder::open(&path), Err(e) if e.kind() == io::ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    }
}
