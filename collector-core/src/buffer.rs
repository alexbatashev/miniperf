use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

pub const BUFFER_BYTES: usize = 256 * 1024;
pub const POOL_BUFFERS: usize = 64;

pub struct Buffer {
    pub data: Vec<u8>,
}

impl Buffer {
    fn new() -> Box<Buffer> {
        Box::new(Buffer {
            data: Vec::with_capacity(BUFFER_BYTES),
        })
    }

    pub fn has_room(&self, bytes: usize) -> bool {
        self.data.len() + bytes <= BUFFER_BYTES
    }
}

struct QueueState {
    full: VecDeque<Box<Buffer>>,
    pool: Vec<Box<Buffer>>,
    closing: bool,
}

/// Bounded buffer pool and handoff queue between producer threads and the
/// writer thread. On pool exhaustion callers drop and count — never block.
pub struct BufferQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl BufferQueue {
    pub fn new() -> BufferQueue {
        BufferQueue {
            state: Mutex::new(QueueState {
                full: VecDeque::new(),
                pool: (0..POOL_BUFFERS).map(|_| Buffer::new()).collect(),
                closing: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// Take an empty buffer, or `None` when the pool is exhausted.
    pub fn acquire(&self) -> Option<Box<Buffer>> {
        self.state.lock().unwrap().pool.pop()
    }

    /// Hand a filled buffer to the writer.
    pub fn submit(&self, buffer: Box<Buffer>) {
        self.state.lock().unwrap().full.push_back(buffer);
        self.ready.notify_one();
    }

    /// Return an empty buffer to the pool.
    pub fn recycle(&self, mut buffer: Box<Buffer>) {
        buffer.data.clear();
        self.state.lock().unwrap().pool.push(buffer);
    }

    /// Writer side: wait for the next filled buffer. `None` once closed and
    /// drained.
    pub fn next_full(&self) -> Option<Box<Buffer>> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(buffer) = state.full.pop_front() {
                return Some(buffer);
            }
            if state.closing {
                return None;
            }
            state = self.ready.wait(state).unwrap();
        }
    }

    pub fn close(&self) {
        self.state.lock().unwrap().closing = true;
        self.ready.notify_all();
    }
}
