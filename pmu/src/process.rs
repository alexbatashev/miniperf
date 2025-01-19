use std::ffi::CString;

#[derive(Debug)]
pub struct Process {
    pid: i32,
    write_fd: i32,
}

impl Process {
    pub fn new(args: &[String], env: &[(String, String)]) -> Result<Self, std::io::Error> {
        let mut pipe_fds: [libc::c_int; 2] = [-1; 2];
        if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } == -1 {
            return Err(std::io::Error::last_os_error());
        }

        let child_pid = unsafe { libc::fork() };
        if child_pid == -1 {
            panic!()
        }

        if child_pid == 0 {
            let prog = CString::new(args[0].clone())?;
            let c_args: Vec<CString> = args
                .iter()
                .map(|arg| CString::new(arg.as_str()).unwrap())
                .collect();
            let mut c_arg_ptrs: Vec<*const libc::c_char> =
                c_args.iter().map(|arg| arg.as_ptr()).collect();
            c_arg_ptrs.push(std::ptr::null());

            let c_env: Vec<CString> = std::env::vars()
                .chain(env.iter().cloned())
                .map(|(key, val)| CString::new(format!("{}={}", key, val)).unwrap())
                .collect();
            let mut c_env_ptrs: Vec<*const libc::c_char> =
                c_env.iter().map(|env| env.as_ptr()).collect();
            c_env_ptrs.push(std::ptr::null());

            // Wait for parent signal
            let mut buf = [0u8; 1];
            unsafe { libc::read(pipe_fds[0], buf.as_mut_ptr() as *mut libc::c_void, 1) };
            unsafe { libc::close(pipe_fds[0]) };

            unsafe {
                if libc::execve(prog.as_ptr(), c_arg_ptrs.as_ptr(), c_env_ptrs.as_ptr()) == -1 {
                    // If we get here, exec failed
                    let err = std::io::Error::last_os_error();
                    eprintln!("excecve failed: {}", err);
                    libc::_exit(1);
                }
            }
        }

        unsafe { libc::close(pipe_fds[0]) };
        Ok(Process {
            pid: child_pid,
            write_fd: pipe_fds[1],
        })
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn cont(&self) {
        unsafe {
            libc::write(self.write_fd, &[1u8] as *const u8 as *const libc::c_void, 1);
            libc::close(self.write_fd);
        }
    }

    pub fn wait(&self) -> Result<(), std::io::Error> {
        unsafe {
            let mut status: libc::c_int = 0;
            if libc::waitpid(self.pid, &mut status, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(())
    }
}
