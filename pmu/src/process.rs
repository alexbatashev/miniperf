use std::ffi::CString;

#[derive(Debug)]
pub struct Process {
    pid: i32,
}

impl Process {
    pub fn new(args: &[String]) -> Result<Self, Box<dyn std::error::Error>> {
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

            unsafe { libc::raise(libc::SIGSTOP) };

            unsafe {
                libc::execvp(prog.as_ptr(), c_arg_ptrs.as_ptr());
                // If we get here, exec failed
                libc::_exit(1);
            }
        }

        Ok(Process { pid: child_pid })
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn cont(&self) {
        unsafe {
            libc::kill(self.pid, libc::SIGCONT);
        }
    }

    pub fn wait(&self) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let mut status: libc::c_int = 0;
            if libc::waitpid(self.pid, &mut status, 0) == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
        }

        Ok(())
    }
}
