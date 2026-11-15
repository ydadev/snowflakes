use super::*;

fn records(sink: &Sink) -> Vec<serde_json::Value> {
    (0..FILES)
        .rev()
        .flat_map(|index| match fs::read_to_string(sink.path(index)) {
            Ok(text) => text
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("cannot read fixture log: {error:?}"),
        })
        .collect()
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready() {
        assert!(Instant::now() < deadline, "log worker did not finish");
        thread::sleep(Duration::from_millis(5));
    }
}
fn record(value: i32) -> Record {
    Record::new(Fault::Graphics, Code::Hresult(value))
}

#[test]
fn five_bounded_files_keep_the_latest_faults_and_append_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Sink {
        directory: dir.path().into(),
        limit: 512,
    };
    fs::write(dir.path().join("keep.dat"), b"unrelated").unwrap();
    let stop = Stop::default();
    for value in 0..80 {
        sink.append(&record(value), &stop).unwrap();
    }
    let restarted = Sink {
        directory: dir.path().into(),
        limit: 512,
    };
    for value in 80..86 {
        restarted.append(&record(value), &stop).unwrap();
    }
    let values = records(&restarted);
    let actual: Vec<_> = values
        .iter()
        .map(|value| value["code"]["value"].as_i64().unwrap())
        .collect();
    assert!(!actual.is_empty() && actual.len() < 86);
    assert_eq!(actual, ((86 - actual.len() as i64)..86).collect::<Vec<_>>());
    for index in 0..FILES {
        assert!(fs::metadata(sink.path(index)).unwrap().len() <= 512);
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), FILES + 2); // Logs, lock, unrelated file.
    assert_eq!(fs::read(dir.path().join("keep.dat")).unwrap(), b"unrelated");
    assert_eq!((FILES, MAX_BYTES), (5, 2_097_152));
}

#[test]
fn exact_two_mebibyte_current_file_rotates_before_adding_a_new_fault() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Sink {
        directory: dir.path().into(),
        limit: MAX_BYTES,
    };
    let mut line = serde_json::to_vec(&record(0)).unwrap();
    line.push(b'\n');
    let mut bytes = line.repeat(MAX_BYTES as usize / line.len());
    let padding = MAX_BYTES as usize - bytes.len();
    // JSON whitespace pads the final valid record to the real production cap.
    bytes.pop();
    bytes.extend(std::iter::repeat_n(b' ', padding));
    bytes.push(b'\n');
    assert_eq!(bytes.len() as u64, MAX_BYTES);
    fs::write(sink.path(0), &bytes).unwrap();
    sink.append(&record(1), &Stop::default()).unwrap();
    assert!(
        fs::read(sink.path(1)).unwrap() == bytes,
        "history changed at the 2 MiB boundary"
    );
    let current: serde_json::Value =
        serde_json::from_slice(&fs::read(sink.path(0)).unwrap()).unwrap();
    assert_eq!(current["code"]["value"], 1);
    assert!(fs::metadata(sink.path(0)).unwrap().len() < MAX_RECORD as u64);
}

#[test]
fn partial_tail_recovers_and_invalid_slots_are_not_truncated_or_removed() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Sink {
        directory: dir.path().into(),
        limit: 512,
    };
    let stop = Stop::default();
    sink.append(&record(0), &stop).unwrap();
    OpenOptions::new()
        .append(true)
        .open(sink.path(0))
        .unwrap()
        .write_all(b"{\"partial\":")
        .unwrap();
    sink.append(&record(1), &stop).unwrap();
    assert_eq!(records(&sink).len(), 2);
    let before = fs::read(sink.path(0)).unwrap();
    fs::create_dir(sink.path(3)).unwrap();
    assert_eq!(
        sink.append(&record(2), &stop).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(sink.path(3).is_dir());
    assert_eq!(fs::read(sink.path(0)).unwrap(), before);
    fs::remove_dir(sink.path(3)).unwrap();
    fs::write(sink.path(3), vec![b'x'; 513]).unwrap();
    assert_eq!(
        sink.append(&record(2), &stop).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(fs::metadata(sink.path(3)).unwrap().len(), 513);
    assert_eq!(fs::read(sink.path(0)).unwrap(), before);
}

#[test]
fn independent_writer_handles_share_one_history_without_interleaved_records() {
    let dir = tempfile::tempdir().unwrap();
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let directory = dir.path().to_path_buf();
            thread::spawn(move || {
                let sink = Sink {
                    directory,
                    limit: MAX_BYTES,
                };
                let stop = Stop::default();
                for value in 0..20 {
                    let deadline = Instant::now() + Duration::from_secs(3);
                    loop {
                        match sink.append(&record(worker * 100 + value), &stop) {
                            Ok(()) => break,
                            Err(error)
                                if error.kind() == io::ErrorKind::WouldBlock
                                    && Instant::now() < deadline => {}
                            Err(error) => panic!("concurrent log failed: {error:?}"),
                        }
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let sink = Sink {
        directory: dir.path().into(),
        limit: MAX_BYTES,
    };
    let mut actual: Vec<_> = records(&sink)
        .iter()
        .map(|r| r["code"]["value"].as_i64().unwrap())
        .collect();
    actual.sort_unstable();
    let expected: Vec<_> = (0..4)
        .flat_map(|worker| (0..20).map(move |i| worker * 100 + i))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn a_locked_sink_cannot_block_producers_and_all_overflow_is_accounted() {
    let dir = tempfile::tempdir().unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(dir.path().join(".errors.lock"))
        .unwrap();
    lock.lock().unwrap();
    let recorder = Recorder::start(dir.path().into());
    let producer = recorder.clone();
    let (sent, done) = mpsc::channel();
    let thread = thread::spawn(move || {
        for value in 0..4096 {
            producer.record(Fault::Graphics, Code::Hresult(value));
        }
        sent.send(()).unwrap();
    });
    // The sink remains locked until after production and shutdown finish.
    done.recv_timeout(Duration::from_secs(3)).unwrap();
    thread.join().unwrap();
    assert!(recorder.snapshot().dropped > 0);
    let counters = recorder.counters.clone();
    drop(recorder);
    wait(|| !counters.running.load(Ordering::Acquire));
    assert_eq!(counters.written.load(Ordering::Relaxed), 0);
    assert_eq!(
        counters.write_errors.load(Ordering::Relaxed) + counters.dropped.load(Ordering::Relaxed),
        4096
    );
    drop(lock);
}

#[test]
fn faults_hide_error_text_and_a_failed_sink_can_recover_without_stopping_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("logs");
    fs::write(&root, b"blocked").unwrap();
    let recorder = Recorder::start(root.clone());
    let error = StoreError::Io(io::Error::other(
        "TEST-PRIVATE configuration and endpoint text",
    ));
    recorder.storage(Fault::ConfigurationWrite, &error);
    wait(|| recorder.snapshot().write_errors == 1);
    assert!(recorder.snapshot().worker_running);
    fs::remove_file(&root).unwrap();
    recorder.storage(Fault::ConfigurationWrite, &error);
    wait(|| recorder.snapshot().written == 1);
    let snapshot = recorder.snapshot();
    assert_eq!(
        (
            snapshot.enqueued,
            snapshot.written,
            snapshot.write_errors,
            snapshot.dropped
        ),
        (2, 1, 1, 0)
    );
    drop(recorder);
    let text = fs::read_to_string(root.join("errors.jsonl")).unwrap();
    assert!(!text.contains("TEST-PRIVATE") && !text.contains("configuration and endpoint"));
    let values = records(&Sink {
        directory: root,
        limit: MAX_BYTES,
    });
    assert_eq!(values.len(), 1);
    let mut keys: Vec<_> = values[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        {
            keys.sort_unstable();
            keys
        },
        [
            "code",
            "fault",
            "schema_version",
            "timestamp_unix_ms",
            "version"
        ]
    );
    assert_eq!(values[0]["fault"], "configuration_write");
    assert_eq!(
        values[0]["code"],
        serde_json::json!({"kind":"storage", "value":"io"})
    );
}
