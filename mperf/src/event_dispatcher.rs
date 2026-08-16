use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};

use mperf_data::{Event, EventType, ProcMapEntry};
use store::{
    ClockAnchorRows, EventKind, EventRows, ModuleRows, PayloadRows, SampleRawRows, SegmentWriter,
    StringInterner, xxh3,
};
use thread_local::ThreadLocal;

/// Routes recorded events into the Parquet session directory: PMU/OS samples
/// into `samples_raw`, instrumentation events into `events`/`payloads`, and
/// module mappings into `modules`. Strings are interned with XXH3-64 IDs.
pub struct EventDispatcher {
    interner: Arc<Mutex<StringInterner>>,
    last_unique_id: ThreadLocal<RefCell<u64>>,
    tx: mpsc::SyncSender<Msg>,
}

enum Msg {
    Event(Event),
    Module(ProcMapEntry),
}

pub struct DispatcherJoinHandle {
    worker: std::thread::JoinHandle<()>,
}

const BATCH_ROWS: usize = 8192;

struct Worker {
    interner: Arc<Mutex<StringInterner>>,
    samples: SampleRawRows,
    samples_writer: SegmentWriter,
    events: EventRows,
    events_writer: SegmentWriter,
    payloads: PayloadRows,
    payload_seen: HashSet<u64>,
    payloads_writer: SegmentWriter,
    modules: HashSet<ProcMapEntry>,
    modules_writer: SegmentWriter,
    clock: ClockAnchorRows,
    clock_writer: SegmentWriter,
}

impl Worker {
    fn new(dir: &Path, interner: Arc<Mutex<StringInterner>>) -> Self {
        let mut clock = ClockAnchorRows::default();
        clock.push("start");
        Worker {
            interner,
            samples: SampleRawRows::default(),
            samples_writer: SegmentWriter::new(dir, "samples_raw", None, SampleRawRows::schema()),
            events: EventRows::default(),
            events_writer: SegmentWriter::new(dir, "events", None, EventRows::schema()),
            payloads: PayloadRows::default(),
            payload_seen: HashSet::new(),
            payloads_writer: SegmentWriter::new(dir, "payloads", None, PayloadRows::schema()),
            modules: HashSet::new(),
            modules_writer: SegmentWriter::new(dir, "modules", None, ModuleRows::schema()),
            clock,
            clock_writer: SegmentWriter::new(dir, "clock", None, ClockAnchorRows::schema()),
        }
    }

    fn intern(&self, value: &str) -> u64 {
        self.interner.lock().unwrap().intern(value)
    }

    fn consume(&mut self, event: Event) {
        if event.ty.is_roofline() {
            self.consume_trace(event);
        } else {
            self.consume_sample(event);
        }
    }

    fn consume_sample(&mut self, event: Event) {
        let event_id = if event.ty == EventType::PmuCustom && event.name != 0 {
            event.name
        } else {
            self.intern(&event.ty.to_string())
        };
        let rows = &mut self.samples;
        rows.timestamp.push(event.timestamp as i64);
        rows.pid.push(event.process_id);
        rows.tid.push(event.thread_id);
        rows.cpu.push(event.cpu);
        rows.group_id.push(event.correlation_id);
        rows.event_id.push(event_id);
        rows.value.push(event.value as i64);
        rows.time_enabled.push(event.time_enabled);
        rows.time_running.push(event.time_running);
        rows.ip.push(
            event
                .callstack
                .first()
                .map(|frame| frame.as_ip())
                .unwrap_or(0),
        );
        rows.callchain
            .push(event.callstack.iter().map(|frame| frame.as_ip()).collect());
        let (abi, mask, regs) = match event.user_regs {
            Some(regs) => (regs.abi, regs.mask, regs.values),
            None => (0, 0, Vec::new()),
        };
        rows.regs_abi.push(abi);
        rows.regs_mask.push(mask);
        rows.regs.push(regs);
        rows.user_stack.push(event.user_stack);
        if rows.len() >= BATCH_ROWS {
            self.flush_samples();
        }
    }

    fn consume_trace(&mut self, event: Event) {
        let (kind, event_id) = match event.ty {
            EventType::RooflineLoopStart => {
                let payload = event.callstack.first().map(|frame| frame.as_loc());
                (EventKind::Begin, self.payload_id(payload))
            }
            EventType::RooflineLoopEnd => (EventKind::End, 0),
            _ => (EventKind::Counter, self.intern(&event.ty.to_string())),
        };
        let rows = &mut self.events;
        rows.timestamp.push(event.timestamp as i64);
        rows.event_id.push(event_id);
        rows.instance.push(event.unique_id);
        rows.parent_id.push(event.parent_id);
        rows.flow_id.push(event.correlation_id);
        rows.kind.push(kind as u8);
        rows.pid.push(event.process_id);
        rows.tid.push(event.thread_id);
        rows.value.push(event.value as i64);
        if rows.len() >= BATCH_ROWS {
            self.flush_events();
        }
    }

    fn payload_id(&mut self, location: Option<mperf_data::Location>) -> u64 {
        let Some(location) = location else {
            return 0;
        };
        let mut bytes = [0u8; 24];
        bytes[..8].copy_from_slice(&location.function_name.to_le_bytes());
        bytes[8..16].copy_from_slice(&location.file_name.to_le_bytes());
        bytes[16..20].copy_from_slice(&location.line.to_le_bytes());
        let id = xxh3(&bytes);
        if self.payload_seen.insert(id) {
            self.payloads.event_id.push(id);
            self.payloads.name_id.push(location.function_name);
            self.payloads.function_id.push(location.function_name);
            self.payloads.file_id.push(location.file_name);
            self.payloads.line.push(location.line);
            self.payloads.column.push(0);
        }
        id
    }

    fn flush_samples(&mut self) {
        if self.samples.is_empty() {
            return;
        }
        let result = self
            .samples
            .to_batch()
            .and_then(|batch| self.samples_writer.write(&batch));
        if let Err(err) = result {
            eprintln!("failed to write samples: {err:#}");
        }
    }

    fn flush_events(&mut self) {
        if self.events.is_empty() {
            return;
        }
        let result = self
            .events
            .to_batch()
            .and_then(|batch| self.events_writer.write(&batch));
        if let Err(err) = result {
            eprintln!("failed to write events: {err:#}");
        }
    }

    fn finish(mut self) {
        self.flush_samples();
        self.flush_events();
        let mut modules = ModuleRows::default();
        for entry in std::mem::take(&mut self.modules) {
            modules.pid.push(entry.pid);
            modules.path.push(entry.filename);
            modules.build_id.push(String::new());
            modules.address.push(entry.address as u64);
            modules.size.push(entry.size as u64);
            modules.offset.push(entry.offset as u64);
        }
        self.clock.push("end");
        let steps = [
            ("payloads", write_all(self.payloads.to_batch(), &mut self.payloads_writer)),
            ("modules", write_all(modules.to_batch(), &mut self.modules_writer)),
            ("clock", write_all(self.clock.to_batch(), &mut self.clock_writer)),
            ("samples_raw", self.samples_writer.finish().map(|_| ())),
            ("events", self.events_writer.finish().map(|_| ())),
            ("strings", self.interner.lock().unwrap().finish()),
        ];
        for (name, result) in steps {
            if let Err(err) = result {
                eprintln!("failed to write {name}: {err:#}");
            }
        }
    }
}

fn write_all(
    batch: anyhow::Result<store::arrow::record_batch::RecordBatch>,
    writer: &mut SegmentWriter,
) -> anyhow::Result<()> {
    let batch = batch?;
    if batch.num_rows() > 0 {
        writer.write(&batch)?;
    }
    writer.finish()?;
    Ok(())
}

impl EventDispatcher {
    pub fn new(output_directory: &Path) -> (Arc<Self>, DispatcherJoinHandle) {
        let interner = Arc::new(Mutex::new(StringInterner::new(output_directory, None)));
        let (tx, rx) = mpsc::sync_channel::<Msg>(BATCH_ROWS);
        let mut worker = Worker::new(output_directory, interner.clone());
        let worker = std::thread::spawn(move || {
            while let Ok(message) = rx.recv() {
                match message {
                    Msg::Event(event) => worker.consume(event),
                    Msg::Module(entry) => {
                        worker.modules.insert(entry);
                    }
                }
            }
            worker.finish();
        });
        (
            Arc::new(EventDispatcher {
                interner,
                last_unique_id: ThreadLocal::new(),
                tx,
            }),
            DispatcherJoinHandle { worker },
        )
    }

    /// A session-unique 64-bit instance ID.
    pub fn unique_id(&self) -> u64 {
        let mut counter = self.last_unique_id.get_or(|| RefCell::new(0)).borrow_mut();
        *counter += 1;
        let mut bytes = [0u8; 20];
        bytes[..4].copy_from_slice(&std::process::id().to_le_bytes());
        bytes[4..12].copy_from_slice(&current_thread_id().to_le_bytes());
        bytes[12..20].copy_from_slice(&counter.to_le_bytes());
        xxh3(&bytes)
    }

    pub fn string_id(&self, string: &str) -> u64 {
        self.interner.lock().unwrap().intern(string)
    }

    pub async fn string_id_async(&self, string: &str) -> u64 {
        self.string_id(string)
    }

    pub fn publish_event_sync(&self, evt: Event) {
        if self.tx.send(Msg::Event(evt)).is_err() {
            eprintln!("lost event: writer stopped");
        }
    }

    pub fn publish_proc_map_sync(&self, map: ProcMapEntry) {
        if self.tx.try_send(Msg::Module(map)).is_err() {
            eprintln!("lost proc map entry: channel full or writer stopped");
        }
    }

    pub async fn publish_event(&self, evt: Event) {
        self.publish_event_sync(evt);
    }
}

fn current_thread_id() -> u64 {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::gettid() as u64 }
    }

    #[cfg(target_os = "macos")]
    {
        let mut tid = 0_u64;
        unsafe {
            libc::pthread_threadid_np(0, &mut tid);
        }
        tid
    }
}

impl DispatcherJoinHandle {
    pub async fn join(self) {
        let worker = self.worker;
        let _ = tokio::task::spawn_blocking(move || worker.join()).await;
    }
}
