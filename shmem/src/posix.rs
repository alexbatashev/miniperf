use std::ffi::c_void;

use libc::{
    c_char, close, ftruncate, mmap, munmap, sem_init, shm_open, shm_unlink, MAP_FAILED, MAP_SHARED,
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
    sem: *mut libc::sem_t,
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
        let name = name.as_ptr() as *const c_char;
        let fd = shm_open(name, flags, S_IRUSR | S_IWUSR);

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
        if unsafe { sem_init(ptr as *mut libc::sem_t, 1, 0) } != 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(Self {
            sem: ptr as *mut libc::sem_t,
            is_owning: true,
        })
    }

    pub fn from_raw_ptr(ptr: *mut ()) -> Result<Self, std::io::Error> {
        Ok(Self {
            sem: ptr as *mut libc::sem_t,
            is_owning: false,
        })
    }

    pub fn required_size() -> usize {
        std::mem::size_of::<libc::sem_t>()
    }

    pub fn wait(&self) -> Result<(), std::io::Error> {
        unsafe {
            if libc::sem_wait(self.sem) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(())
    }

    pub fn try_wait(&self) -> Result<(), std::io::Error> {
        unsafe {
            if libc::sem_trywait(self.sem) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(())
    }

    pub fn post(&self) -> Result<(), std::io::Error> {
        unsafe {
            if libc::sem_post(self.sem) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(())
    }

    pub fn counter(&self) -> Result<i32, std::io::Error> {
        let mut res = 0;
        unsafe {
            if libc::sem_getvalue(self.sem, &mut res as *mut i32) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(res)
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        if self.is_owning {
            unsafe {
                libc::sem_destroy(self.sem);
            }
        }
    }
}

unsafe impl Send for Shmem {}
unsafe impl Send for Semaphore {}

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

        semaphore1.post().expect("failed to post a semaphore");

        assert_eq!(semaphore1.counter().unwrap(), 1);

        let semaphore2 =
            Semaphore::from_raw_ptr(shmem2.as_mut_ptr()).expect("failed to open a semaphore");

        assert_eq!(semaphore2.counter().unwrap(), 1);

        assert!(semaphore2.wait().is_ok());
        assert_eq!(semaphore2.counter().unwrap(), 0);
        assert_eq!(semaphore1.counter().unwrap(), 0);
    }
}
