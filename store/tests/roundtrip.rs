use store::{
    EventKind, EventRows, SampleRows, SegmentWriter, Session, StackRows, StringInterner,
    stack_hash, table_base_name,
};

#[test]
fn base_names_strip_pid_and_seq() {
    assert_eq!(table_base_name("events-1234-0"), "events");
    assert_eq!(table_base_name("event_meta-1234-2"), "event_meta");
    assert_eq!(table_base_name("clock-99"), "clock");
    assert_eq!(table_base_name("stacks"), "stacks");
    assert_eq!(table_base_name("snapshot_resource_samples"), "snapshot_resource_samples");
}

#[test]
fn writes_segments_and_queries_them_back() {
    let dir = tempfile::tempdir().unwrap();

    let mut interner = StringInterner::new(dir.path(), Some(42));
    let cycles = interner.intern("pmu_cycles");
    assert_eq!(cycles, interner.intern("pmu_cycles"));
    interner.finish().unwrap();

    let mut samples = SampleRows::default();
    let mut stacks = StackRows::default();
    let frames = vec![0x1000u64, 0x2000, 0x3000];
    let stack_id = stack_hash(&frames);
    stacks.stack_id.push(stack_id);
    stacks.frames.push(frames);
    for i in 0..1000i64 {
        samples.timestamp.push(i);
        samples.pid.push(42);
        samples.tid.push(43);
        samples.cpu.push(0);
        samples.group_id.push(1);
        samples.event_id.push(cycles);
        samples.value.push(100);
        samples.time_enabled.push(1);
        samples.time_running.push(1);
        samples.ip.push(0x1000);
        samples.stack_id.push(stack_id);
    }
    let mut writer = SegmentWriter::new(dir.path(), "samples", Some(42), SampleRows::schema())
        .with_segment_bytes(1);
    writer.write(&samples.to_batch().unwrap()).unwrap();
    let files = writer.finish().unwrap();
    assert_eq!(files.len(), 1);

    let mut writer = SegmentWriter::new(dir.path(), "stacks", None, StackRows::schema());
    writer.write(&stacks.to_batch().unwrap()).unwrap();
    writer.finish().unwrap();

    let mut events = EventRows::default();
    events.timestamp.push(5);
    events.event_id.push(7);
    events.instance.push(1);
    events.parent_id.push(0);
    events.flow_id.push(0);
    events.kind.push(EventKind::Instant as u8);
    events.pid.push(42);
    events.tid.push(43);
    events.value.push(0);
    let mut writer = SegmentWriter::new(dir.path(), "events", Some(42), EventRows::schema());
    writer.write(&events.to_batch().unwrap()).unwrap();
    writer.finish().unwrap();

    let session = Session::open(dir.path()).unwrap();
    assert!(session.has_table("samples"));
    assert!(session.has_table("strings"));
    assert!(session.has_table("events"));

    let total: i64 = session
        .connection()
        .query_row(
            "SELECT SUM(s.value)::BIGINT FROM samples s \
             JOIN strings n ON n.id = s.event_id WHERE n.string = 'pmu_cycles'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, 100_000);

    let frame_count: i64 = session
        .connection()
        .query_row(
            "SELECT len(frames)::BIGINT FROM stacks WHERE stack_id = ?",
            [stack_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(frame_count, 3);
}
