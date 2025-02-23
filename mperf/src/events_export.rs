use std::{fs::File, path::Path};

use memmap2::{Advice, Mmap};
use mperf_data::Event;

pub fn do_events_export(path: &Path) {
    let events_path = path.join("events.bin");
    let file = File::open(events_path).expect("failed to open events.bin");

    let map = unsafe { Mmap::map(&file).expect("failed to map events.bin to memory") };
    map.advise(Advice::Sequential)
        .expect("Failed to advice sequential reads");

    let mut events = vec![];

    let data_stream = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };

    let mut cursor = std::io::Cursor::new(data_stream);

    while (cursor.position() as usize) < map.len() {
        let evt = Event::read_binary(&mut cursor).expect("Failed to decode event");
        events.push(evt);
    }

    println!("{}", serde_json::to_string_pretty(&events).unwrap());
}
