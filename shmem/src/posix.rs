use std::{
    ffi::{c_void, CString},
    future::Future,
    hint::spin_loop,
    sync::atomic::{AtomicU64, Ordering},
    task::Poll,
};

use libc::{
    close, ftruncate, mmap, munmap, sem_init, shm_open, shm_unlink, MAP_FAILED, MAP_SHARED,
    O_CREAT, O_RDWR, PROT_WRITE, S_IRUSR, S_IWUSR,
};

pub struct Shmem {
    ptr: *mut libc::c_void,
    fd: i32,
    is_owning: bool,
    size: usize,
    name: String,
}

pub struct Semaphore {
    sem: *mut u64,
    is_owning: bool,
}

impl Shmem {
    pub fn create(name: &str, size: usize) -> Result<Self, std::io::Error> {
        let (fd, ptr) = unsafe { Self::raw_parts(name, size, O_RDWR | O_CREAT)? };

        Ok(Self {
            ptr,
            fd,
            is_owning: true,
            size,
            name: name.to_owned(),
        })
    }

    pub fn open(name: &str, size: usize) -> Result<Self, std::io::Error> {
        let (fd, ptr) = unsafe { Self::raw_parts(name, size, O_RDWR)? };

        Ok(Self {
            ptr,
            fd,
            is_owning: false,
            size,
            name: name.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn as_ptr(&self) -> *const () {
        self.ptr as *const ()
    }

    pub fn as_mut_ptr(&self) -> *mut () {
        self.ptr as *mut ()
    }

    unsafe fn raw_parts(
        name: &str,
        size: usize,
        flags: i32,
    ) -> Result<(i32, *mut c_void), std::io::Error> {
        let name = CString::new(name)?;
        let fd = shm_open(name.as_ptr(), flags, (S_IRUSR | S_IWUSR) as libc::c_uint);

        if fd == -1 {
            eprintln!("fd == -1\n");
            return Err(std::io::Error::last_os_error());
        }

        if ftruncate(fd, size as i64) == -1 {
            eprintln!("ftruncate fail\n");
            return Err(std::io::Error::last_os_error());
        }

        let addr = mmap(std::ptr::null_mut(), size, PROT_WRITE, MAP_SHARED, fd, 0);

        if addr == MAP_FAILED {
            eprintln!("mmap failed");
            return Err(std::io::Error::last_os_error());
        }

        Ok((fd, addr))
    }
}

impl Drop for Shmem {
    fn drop(&mut self) {
        unsafe {
            munmap(self.ptr, self.size);
            close(self.fd);
        }
        if self.is_owning {
            unsafe {
                let name = self.name.as_str().as_ptr() as *const libc::c_char;
                shm_unlink(name);
            }
        }
    }
}

impl Semaphore {
    pub fn create(ptr: *mut ()) -> Result<Self, std::io::Error> {
        let atomic = unsafe { AtomicU64::from_ptr(ptr as *mut u64) };

        atomic.store(0, Ordering::SeqCst);

        Ok(Self {
            sem: ptr as *mut u64,
            is_owning: true,
        })
    }

    pub fn from_raw_ptr(ptr: *mut ()) -> Result<Self, std::io::Error> {
        Ok(Self {
            sem: ptr as *mut u64,
            is_owning: false,
        })
    }

    pub fn required_size() -> usize {
        std::mem::size_of::<AtomicU64>()
    }

    pub fn wait_sync(&self) {
        let atomic = unsafe { AtomicU64::from_ptr(self.sem) };

        while atomic.load(Ordering::Acquire) == 0 {
            waste();
        }

        atomic.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn wait(&self) -> impl Future<Output = ()> {
        AtomicFuture::new(self.sem)
    }

    pub fn post(&self) {
        let atomic = unsafe { AtomicU64::from_ptr(self.sem) };
        atomic.fetch_add(1, Ordering::AcqRel);
    }

    pub fn counter(&self) -> u64 {
        let atomic = unsafe { AtomicU64::from_ptr(self.sem) };
        atomic.load(Ordering::Acquire)
    }
}

unsafe impl Send for Shmem {}
unsafe impl Send for Semaphore {}
unsafe impl Send for AtomicFuture {}

#[inline]
fn waste() {
    for _ in 0..100 {
        spin_loop()
    }
}

struct AtomicFuture {
    ptr: *mut u64,
}

impl AtomicFuture {
    fn new(ptr: *mut u64) -> Self {
        Self { ptr }
    }
}

impl Future for AtomicFuture {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let atomic = unsafe { AtomicU64::from_ptr(self.ptr) };
        if atomic.load(Ordering::Acquire) == 0 {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Semaphore, Shmem};

    #[test]
    fn shmem_updates() {
        let name = format!("/shmem_updates_{}", std::process::id());
        let shmem1 = Shmem::create(&name, 32).expect("failed to create shmem");
        let shmem2 = Shmem::open(&name, 32).expect("failed to open shmem");

        unsafe {
            *(shmem1.as_mut_ptr() as *mut i32) = 42;
        };

        let result = unsafe { *(shmem2.as_ptr() as *const i32) };

        assert_eq!(result, 42);
    }

    #[test]
    fn shared_semaphores() {
        let name = format!("/shared_semaphores_{}", std::process::id());
        let shmem1 = Shmem::create(&name, 32).expect("failed to create shmem");
        let shmem2 = Shmem::open(&name, 32).expect("failed to open shmem");

        let semaphore1 =
            Semaphore::create(shmem1.as_mut_ptr()).expect("failed to create a semaphore");

        semaphore1.post();

        assert_eq!(semaphore1.counter(), 1);

        let semaphore2 =
            Semaphore::from_raw_ptr(shmem2.as_mut_ptr()).expect("failed to open a semaphore");

        assert_eq!(semaphore2.counter(), 1);

        semaphore2.wait();
        assert_eq!(semaphore2.counter(), 0);
        assert_eq!(semaphore1.counter(), 0);
    }
}
