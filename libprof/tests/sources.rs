//! Sources are exercised end to end through the sink an embedder would write.
//!
//! None of this needs perf privileges: it drives the resource sources against
//! the test process itself and asserts on the records that come out.

use std::sync::{Arc, Mutex};

use libprof::{Availability, Record, ResourceSample, SessionContext, Sink, Source, SourceStatus};

#[derive(Default)]
struct Collected {
    resources: Vec<ResourceSample>,
    processes: Vec<libprof::ProcessInfo>,
    metrics: Vec<(String, f64)>,
}

#[derive(Default)]
struct Recorder(Mutex<Collected>);

impl Sink for Recorder {
    fn record(&self, record: Record) {
        let mut collected = self.0.lock().unwrap();
        match record {
            Record::Resource(sample) => collected.resources.push(sample),
            Record::Process(info) => collected.processes.push(info),
            Record::Metric { name, value, .. } => collected.metrics.push((name, value)),
            Record::Sample(_) | Record::MemSample(_) | Record::ProcAddr(_) => {}
        }
    }
}

/// Run a source against this test process for long enough to produce a tick.
fn run(source: &mut dyn Source) -> (Collected, Vec<SourceStatus>) {
    let directory = std::env::temp_dir();
    let sink = Arc::new(Recorder::default());
    let context = SessionContext {
        directory,
        sink: sink.clone(),
        process: None,
        attached_pid: Some(std::process::id()),
    };
    source.start(&context).expect("source starts");
    std::thread::sleep(std::time::Duration::from_millis(1_200));
    let statuses = source.stop(&context);
    let collected = std::mem::take(&mut *sink.0.lock().unwrap());
    (collected, statuses)
}

#[test]
fn procfs_source_reports_the_process_tree_and_its_resources() {
    let mut source = libprof::ProcfsSource::default();
    let directory = std::env::temp_dir();
    if let Availability::Unavailable { reason } = source.probe(&directory) {
        // A host without procfs is exactly the case the source must survive.
        assert!(!reason.is_empty());
        return;
    }

    let (collected, statuses) = run(&mut source);

    let metrics: Vec<&str> = collected
        .resources
        .iter()
        .map(|sample| sample.metric.as_str())
        .collect();
    for expected in ["user_time", "system_time", "rss", "read_bytes"] {
        assert!(
            metrics.contains(&expected),
            "missing {expected}: {metrics:?}"
        );
    }
    assert!(
        collected
            .resources
            .iter()
            .all(|sample| !sample.unit.is_empty() && !sample.quality.is_empty()),
        "every observation carries its unit and provenance"
    );
    assert!(
        collected
            .processes
            .iter()
            .any(|process| process.pid == std::process::id()),
        "the test process is in its own tree"
    );
    assert!(
        statuses.iter().any(|status| status.name == "process_tree"),
        "the source reports how it observed the tree: {statuses:?}"
    );
}

#[test]
fn host_telemetry_source_always_reports_what_it_could_read() {
    let mut source = libprof::HostTelemetrySource::default();
    let (collected, statuses) = run(&mut source);

    let status = statuses
        .iter()
        .find(|status| status.name == "host_telemetry")
        .expect("the telemetry source always reports itself");
    if status.is_available() {
        assert!(
            !collected.resources.is_empty(),
            "an available sensor set produces observations"
        );
        assert!(
            collected
                .resources
                .iter()
                .all(|sample| sample.value.is_finite()),
            "a reading that is not a number is not a reading"
        );
    } else {
        // A host with no clock or thermal sensors says why, and says it once.
        assert!(!status.message.is_empty(), "{status:?}");
    }
}

#[test]
fn unavailable_sources_explain_themselves_rather_than_failing_to_build() {
    let directory = std::env::temp_dir();
    for source in [
        Box::new(libprof::BpfSource::default()) as Box<dyn Source>,
        Box::new(libprof::ProcfsSource::default()),
        Box::new(libprof::PreciseMemorySource::default()),
    ] {
        if let Availability::Unavailable { reason } = source.probe(&directory) {
            assert!(
                !reason.is_empty(),
                "'{}' must say why it cannot run",
                source.declare().name
            );
        }
    }
}
