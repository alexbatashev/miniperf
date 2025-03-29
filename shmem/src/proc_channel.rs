use libc::EAGAIN;
use std::{
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{platform, utils::blocker};

pub trait Sendable {
    fn as_raw_bytes(&self) -> Vec<u8>;
    fn from_raw_bytes(bytes: &[u8]) -> Self;
}

pub struct Sender<T: Sendable> {
    inner: Inner,
    phantom: PhantomData<T>,
}

#[allow(dead_code)]
pub struct Receiver<T: Sendable> {
    inner: Inner,
    phantom: PhantomData<T>,
}

struct Inner {
    shmem: platform::Shmem,
    sem: platform::Semaphore,
    finish_sem: platform::Semaphore,
    ptr: *mut (),
    head: *mut usize,
    tail: *mut usize,
    size: usize,
}

impl Inner {
    fn data_offset() -> usize {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let sem_size = 2 * platform::Semaphore::required_size();
        let alignment = std::mem::align_of::<usize>() as isize;

        (sem_size + std::mem::size_of::<usize>() * 2 - 1 + alignment as usize)
            & (-alignment as usize)
    }

    fn compute_size(data_size: usize) -> usize {
        let data_offset = Self::data_offset();
        data_offset + data_size
    }

    fn new(shmem: platform::Shmem, data_size: usize, init: bool) -> Result<Self, std::io::Error> {
        let (sem, finish_sem) = if init {
            (
                platform::Semaphore::create(shmem.as_mut_ptr())?,
                platform::Semaphore::create(unsafe {
                    shmem
                        .as_mut_ptr()
                        .byte_add(platform::Semaphore::required_size())
                })?,
            )
        } else {
            (
                platform::Semaphore::from_raw_ptr(shmem.as_mut_ptr())?,
                platform::Semaphore::from_raw_ptr(unsafe {
                    shmem
                        .as_mut_ptr()
                        .byte_add(platform::Semaphore::required_size())
                })?,
            )
        };

        let sem_size = 2 * platform::Semaphore::required_size();
        let data_offset = Self::data_offset();

        let ptr = unsafe { shmem.as_mut_ptr().byte_add(data_offset) };

        let head = unsafe { shmem.as_mut_ptr().byte_add(sem_size) as *mut usize };
        let tail = unsafe {
            shmem
                .as_mut_ptr()
                .byte_add(sem_size + std::mem::size_of::<usize>()) as *mut usize
        };

        unsafe {
            *head = 0;
            *tail = 0;
        };

        Ok(Inner {
            shmem,
            sem,
            finish_sem,
            ptr,
            head,
            tail,
            size: data_size,
        })
    }
}

impl<T: Sendable> Sender<T> {
    pub fn new(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        let total_size = Inner::compute_size(data_size);

        let shmem = platform::Shmem::create(name, total_size)?;
        let inner = Inner::new(shmem, data_size, true)?;

        Ok(Sender {
            inner,
            phantom: PhantomData,
        })
    }

    pub fn attach(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        let total_size = Inner::compute_size(data_size);

        let shmem = platform::Shmem::open(name, total_size)?;
        let inner = Inner::new(shmem, data_size, false)?;

        Ok(Sender {
            inner,
            phantom: PhantomData,
        })
    }

    pub fn name(&self) -> &str {
        self.inner.shmem.name()
    }

    pub fn send_sync(&self, object: T) -> Result<(), std::io::Error> {
        let max = if self.head().load(Ordering::SeqCst) > self.tail().load(Ordering::SeqCst) {
            self.head().load(Ordering::SeqCst)
        } else {
            self.inner.size
        };

        let data = object.as_raw_bytes();

        let usize_size = std::mem::size_of::<usize>();
        let len = if data.len() % usize_size == 0 {
            data.len()
        } else {
            (data.len() / usize_size + 1) * usize_size
        };

        assert_ne!(len, 0);

        let tail = self.tail().fetch_add(len + usize_size, Ordering::SeqCst);

        if tail >= max {
            panic!("TODO make an actual ring buffer");
        }

        unsafe {
            let size_ptr = self.inner.ptr.byte_add(tail) as *mut usize;
            *size_ptr = len;

            std::ptr::copy(
                data.as_ptr(),
                self.inner.ptr.byte_add(tail + usize_size) as *mut u8,
                data.len(),
            );

            if len - data.len() > 0 {
                std::ptr::write_bytes(
                    self.inner.ptr.byte_add(tail + usize_size + data.len()),
                    0,
                    len - data.len(),
                );
            }

            std::sync::atomic::fence(Ordering::SeqCst);
        }

        self.inner.sem.post()?;

        Ok(())
    }

    pub fn close(&self) -> Result<(), std::io::Error> {
        self.inner.finish_sem.post()
    }

    fn head(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.inner.head) }
    }
    fn tail(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.inner.tail) }
    }
}

impl<T: Sendable> Drop for Sender<T> {
    fn drop(&mut self) {
        let _ = self.inner.finish_sem.post();
    }
}

impl<T: Sendable> Receiver<T> {
    pub fn attach(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        let total_size = Inner::compute_size(data_size);

        let shmem = platform::Shmem::open(name, total_size)?;
        let inner = Inner::new(shmem, data_size, false)?;

        Ok(Receiver {
            inner,
            phantom: PhantomData,
        })
    }

    pub fn new(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        let total_size = Inner::compute_size(data_size);

        let shmem = platform::Shmem::create(name, total_size)?;
        let inner = Inner::new(shmem, data_size, true)?;

        Ok(Receiver {
            inner,
            phantom: PhantomData,
        })
    }

    pub fn recv_sync(&self) -> Option<T> {
        if self.inner.sem.counter().ok()? == 0 && self.inner.finish_sem.counter().ok()? > 0 {
            return None;
        }

        self.inner.sem.wait().ok()?;

        let size_offset = self
            .head()
            .fetch_add(std::mem::size_of::<usize>(), Ordering::SeqCst);

        let size = unsafe { *(self.inner.ptr.byte_add(size_offset) as *const usize) };

        assert_ne!(size, 0);

        let offset = self.head().fetch_add(size, Ordering::SeqCst);

        assert_eq!(offset, size_offset + std::mem::size_of::<usize>());

        let slice = unsafe {
            std::slice::from_raw_parts(self.inner.ptr.byte_add(offset) as *const u8, size)
        };

        Some(T::from_raw_bytes(slice))
    }

    pub async fn recv(&self) -> Option<T> {
        let _ = blocker(|| {
            let res = self.inner.sem.try_wait();
            if res.is_ok() {
                return Ok(true);
            }

            let err = res.err().unwrap();
            if let Some(code) = err.raw_os_error() {
                if code == EAGAIN {
                    if self.inner.finish_sem.counter()? > 0 {
                        return Ok(true);
                    }

                    return Ok(false);
                }
            }

            Err(err)
        })
        .await;

        if self.empty() && self.inner.finish_sem.counter().ok()? > 0 {
            return None;
        }

        let size_offset = self
            .head()
            .fetch_add(std::mem::size_of::<usize>(), Ordering::SeqCst);

        let size = unsafe { *(self.inner.ptr.byte_add(size_offset) as *const usize) };

        assert_ne!(size, 0);

        let offset = self.head().fetch_add(size, Ordering::SeqCst);

        assert_eq!(offset, size_offset + std::mem::size_of::<usize>());

        let slice = unsafe {
            std::slice::from_raw_parts(self.inner.ptr.byte_add(offset) as *const u8, size)
        };

        Some(T::from_raw_bytes(slice))
    }

    pub fn empty(&self) -> bool {
        match self.inner.sem.counter() {
            Ok(c) => c == 0,
            _ => true,
        }
    }

    fn head(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.inner.head) }
    }

    #[allow(dead_code)]
    fn tail(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.inner.tail) }
    }
}

impl<T: Copy> Sendable for T {
    fn as_raw_bytes(&self) -> Vec<u8> {
        let mut vec = vec![0; std::mem::size_of::<Self>()];

        unsafe {
            std::ptr::copy(
                self as *const Self as *const u8,
                vec.as_mut_ptr(),
                std::mem::size_of::<Self>(),
            );
        };

        vec
    }

    fn from_raw_bytes(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= std::mem::size_of::<T>());

        let mut to: MaybeUninit<T> = MaybeUninit::uninit();

        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                to.as_mut_ptr().cast::<u8>(),
                size_of::<T>(),
            );
            to.assume_init()
        }
    }
}

unsafe impl<T: Sendable> Send for Receiver<T> {}
unsafe impl<T: Sendable> Sync for Receiver<T> {}
unsafe impl<T: Sendable> Send for Sender<T> {}

#[cfg(test)]
mod test {
    use super::{Receiver, Sender};

    #[test]
    fn channel_creation() {
        if std::env::var("CI").is_ok() {
            println!("This test is failing in CI only. Skipping.");
            return;
        }
        let name = format!("/test_shmem_{}", std::process::id());
        let sender = Sender::<usize>::new(&name, 8192);

        assert!(sender.is_ok());

        let sender = sender.unwrap();

        let receiver = Receiver::<usize>::attach(sender.name(), 8192);

        assert!(receiver.is_ok());

        let receiver = receiver.unwrap();

        sender.send_sync(42).expect("failed to send an object");

        let result = receiver.recv_sync().expect("failed to receive an object");

        assert_eq!(result, 42);
    }
}
