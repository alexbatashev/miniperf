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

#[allow(dead_code)]
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
    pub fn new(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let sem_size = platform::Semaphore::required_size();
        let alignment = std::mem::align_of::<usize>() as isize;

        let data_offset = (sem_size + std::mem::size_of::<usize>() * 2 - 1 + alignment as usize)
            & (-alignment as usize);
        let total_size = data_offset + data_size;

        let shmem = platform::Shmem::create(name, total_size)?;
        let tx_sem = platform::Semaphore::create(shmem.as_mut_ptr())?;
        let ptr = unsafe { shmem.as_mut_ptr().byte_add(data_offset) };

        eprintln!("[new] sender_ptr = {:?}", ptr);

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
            size: data_size,
            phantom: PhantomData,
        })
    }

    pub fn attach(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let sem_size = platform::Semaphore::required_size();
        let alignment = std::mem::align_of::<usize>() as isize;

        let data_offset = (sem_size + std::mem::size_of::<usize>() * 2 - 1 + alignment as usize)
            & (-alignment as usize);
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
            size: data_size,
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

        // let data = object.as_raw_bytes();
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];

        let usize_size = std::mem::size_of::<usize>();
        let len = if data.len() % usize_size == 0 {
            data.len()
        } else {
            (data.len() / usize_size + 1) * usize_size
        };

        assert_ne!(len, 0);

        let tail = self.tail().fetch_add(len + usize_size, Ordering::SeqCst);

        eprintln!("tail = {}", tail);
        eprintln!("len = {}", data.len());

        for (idx, val) in data.iter().enumerate() {
            eprintln!("data[{}] = {}", idx, val);
        }

        if tail >= max {
            panic!("TODO make an actual ring buffer");
        }

        unsafe {
            let size_ptr = self.ptr.byte_add(tail) as *mut usize;
            *size_ptr = len;

            std::ptr::copy(
                data.as_ptr(),
                self.ptr.byte_add(tail + usize_size) as *mut u8,
                data.len(),
            );

            eprintln!("ptr = {:?}", size_ptr);

            if len - data.len() > 0 {
                std::ptr::write_bytes(
                    self.ptr.byte_add(tail + usize_size + data.len()),
                    0,
                    len - data.len(),
                );
            }

            std::sync::atomic::fence(Ordering::SeqCst);

            for i in 0..len {
                unsafe {
                    let ptr = self.ptr.byte_add(tail + usize_size + i) as *const u8;
                    eprintln!("slice[{}] = {}", i, *ptr);
                }
            }
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
    pub fn attach(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let sem_size = platform::Semaphore::required_size() * 2;
        let alignment = std::mem::align_of::<usize>() as isize;

        let data_offset = (sem_size - 1 + alignment as usize) & (-alignment as usize);
        let total_size = data_offset + data_size;

        let shmem = platform::Shmem::open(name, total_size)?;
        let tx_sem = platform::Semaphore::from_raw_ptr(shmem.as_mut_ptr())?;
        let ptr = unsafe { shmem.as_mut_ptr().byte_add(data_offset) };

        eprintln!("[attach] recv_ptr = {:?}", ptr);

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
            size: data_size,
            phantom: PhantomData,
        })
    }

    pub fn new(name: &str, data_size: usize) -> Result<Self, std::io::Error> {
        assert_eq!(
            platform::Semaphore::required_size() % std::mem::align_of::<usize>(),
            0
        );
        let sem_size = platform::Semaphore::required_size() * 2;
        let alignment = std::mem::align_of::<usize>() as isize;

        let data_offset = (sem_size - 1 + alignment as usize) & (-alignment as usize);
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

        Ok(Receiver {
            shmem,
            tx_sem,
            ptr,
            head,
            tail,
            size: data_size,
            phantom: PhantomData,
        })
    }

    pub fn recv_sync(&self) -> Result<T, std::io::Error> {
        self.tx_sem.wait()?;

        for i in 0..16 {
            unsafe {
                let data = self.ptr.byte_add(i * std::mem::size_of::<usize>()) as *const usize;
                eprintln!("recv_ptr[{}] = {} ({:?})", i, *data, data);
            }
        }

        let size_offset = self
            .head()
            .fetch_add(std::mem::size_of::<usize>(), Ordering::SeqCst);

        let size = unsafe { *(self.ptr.byte_add(size_offset) as *const usize) };

        eprintln!("recv_ptr = {:?}", unsafe { self.ptr.byte_add(size_offset) });

        eprintln!("recv_size_offset = {}", size_offset);
        eprintln!("recv_size = {}", size);

        let offset = self.head().fetch_add(size, Ordering::SeqCst);

        eprintln!("recv_offset = {}", offset);

        assert_eq!(offset, size_offset + std::mem::size_of::<usize>());

        let slice =
            unsafe { std::slice::from_raw_parts(self.ptr.byte_add(offset) as *const u8, size) };

        for (idx, val) in slice.iter().enumerate() {
            eprintln!("recv_slice[{}] = {}", idx, val);
        }

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

        let size_offset = self
            .head()
            .fetch_add(std::mem::size_of::<usize>(), Ordering::SeqCst);

        let size = unsafe { *(self.ptr.byte_add(size_offset) as *const usize) };

        let offset = self.head().fetch_add(size, Ordering::SeqCst);

        assert_eq!(offset, size_offset + std::mem::size_of::<usize>());

        let slice =
            unsafe { std::slice::from_raw_parts(self.ptr.byte_add(offset) as *const u8, size) };

        Ok(T::from_raw_bytes(slice))
    }

    fn head(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.head) }
    }

    #[allow(dead_code)]
    fn tail(&self) -> &AtomicUsize {
        unsafe { AtomicUsize::from_ptr(self.tail) }
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
        // assert_eq!(bytes.len(), std::mem::size_of::<T>());

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
        let name = format!("/test_shmem_{}", std::process::id());
        let sender = Sender::<usize>::new(&name, 8192);

        if sender.is_err() {
            eprintln!("{:?}", &sender.as_ref().err());
        }
        assert!(sender.is_ok());

        let sender = sender.unwrap();

        let receiver = Receiver::<usize>::attach(sender.name(), 8192);

        assert!(receiver.is_ok());

        let res = sender.send_sync(42);
        assert!(res.is_ok());

        let receiver = receiver.unwrap();
        let res = receiver.recv_sync();
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), 42);
    }
}
