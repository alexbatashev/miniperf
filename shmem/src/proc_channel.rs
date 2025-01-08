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
    shmem: platform::Shmem,
    tx_sem: platform::Semaphore,
    ptr: *mut (),
    head: *mut usize,
    tail: *mut usize,
    size: usize,
    phantom: PhantomData<T>,
}

pub struct Receiver<T: Sendable> {
    shmem: platform::Shmem,
    tx_sem: platform::Semaphore,
    ptr: *mut (),
    head: *mut usize,
    tail: *mut usize,
    size: usize,
    phantom: PhantomData<T>,
}

impl<T: Sendable> Sender<T> {
    pub fn create(name: &str, count: usize) -> Result<Self, std::io::Error> {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let data_size = std::mem::size_of::<T>() * count;
        let sem_size = platform::Semaphore::required_size();
        let alignment = std::mem::align_of::<T>() as isize;

        let data_offset = (sem_size + std::mem::size_of::<usize>() * 2 - 1 + alignment as usize)
            & (-alignment as usize);
        let total_size = data_offset + data_size;

        let shmem = platform::Shmem::create(name, total_size)?;
        let tx_sem = platform::Semaphore::create(shmem.as_mut_ptr())?;
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

        Ok(Sender {
            shmem,
            tx_sem,
            ptr,
            head,
            tail,
            size: count,
            phantom: PhantomData,
        })
    }

    pub fn name(&self) -> &str {
        self.shmem.name()
    }

    pub fn send_sync(&self, object: T) -> Result<(), std::io::Error> {
        let max = if self.head().load(Ordering::SeqCst) > self.tail().load(Ordering::SeqCst) {
            self.head().load(Ordering::SeqCst)
        } else {
            self.size
        };

        let data = object.as_raw_bytes();

        let tail = self.tail().fetch_add(1, Ordering::SeqCst);

        if tail >= max {
            panic!("TODO make an actual ring buffer");
        }

        unsafe {
            std::ptr::copy(
                data.as_ptr(),
                self.ptr.byte_add(tail) as *mut u8,
                data.len(),
            );
        }

        self.tx_sem.post()?;

        Ok(())
    }

    fn head(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.head) }
    }
    fn tail(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.tail) }
    }
}

impl<T: Sendable> Receiver<T> {
    pub fn create(name: &str, count: usize) -> Result<Self, std::io::Error> {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let data_size = std::mem::size_of::<T>() * count;
        let sem_size = platform::Semaphore::required_size() * 2;
        let alignment = std::mem::align_of::<T>() as isize;

        let data_offset = (sem_size - 1 + alignment as usize) & (-alignment as usize);
        let total_size = data_offset + data_size;

        let shmem = platform::Shmem::open(name, total_size)?;
        let tx_sem = platform::Semaphore::from_raw_ptr(shmem.as_mut_ptr())?;
        let ptr = unsafe { shmem.as_mut_ptr().byte_add(data_offset) };

        let head = unsafe { shmem.as_mut_ptr().byte_add(sem_size) as *mut usize };
        let tail = unsafe {
            shmem
                .as_mut_ptr()
                .byte_add(sem_size + std::mem::size_of::<usize>()) as *mut usize
        };

        Ok(Receiver {
            shmem,
            tx_sem,
            ptr,
            head,
            tail,
            size: count,
            phantom: PhantomData,
        })
    }

    pub fn recv_sync(&self) -> Result<T, std::io::Error> {
        self.tx_sem.wait()?;
        let offset = self.head().fetch_add(1, Ordering::SeqCst) * std::mem::size_of::<T>();

        let slice = unsafe {
            std::slice::from_raw_parts(
                self.ptr.byte_add(offset) as *const u8,
                std::mem::size_of::<T>(),
            )
        };

        Ok(T::from_raw_bytes(slice))
    }

    pub async fn recv(&self) -> Result<T, std::io::Error> {
        blocker(|| {
            let res = self.tx_sem.try_wait();
            if res.is_ok() {
                return Ok(true);
            }

            let err = res.err().unwrap();
            if let Some(code) = err.raw_os_error() {
                if code == EAGAIN {
                    return Ok(false);
                }
            }

            Err(err)
        })
        .await?;

        let offset = self.head().fetch_add(1, Ordering::SeqCst) * std::mem::size_of::<T>();

        let slice = unsafe {
            std::slice::from_raw_parts(
                self.ptr.byte_add(offset) as *const u8,
                std::mem::size_of::<T>(),
            )
        };

        Ok(T::from_raw_bytes(slice))
    }

    fn head(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.head) }
    }

    fn tail(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.tail) }
    }
}

impl<T: Copy> Sendable for T {
    fn as_raw_bytes(&self) -> Vec<u8> {
        let mut vec = Vec::with_capacity(std::mem::size_of::<Self>());

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
        assert_eq!(bytes.len(), std::mem::size_of::<T>());

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
unsafe impl<T: Sendable> Send for Sender<T> {}

#[cfg(test)]
mod test {
    use super::{Receiver, Sender};

    #[test]
    fn channel_creation() {
        let sender = Sender::<i32>::create("/mperf_test_shmem", 10);

        assert!(sender.is_ok());

        let sender = sender.unwrap();

        let receiver = Receiver::<i32>::create(sender.name(), 10);

        assert!(receiver.is_ok());
    }
}
