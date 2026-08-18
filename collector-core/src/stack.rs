pub const MAX_FRAMES: usize = 64;

/// Best-effort frame-pointer walk of the calling thread's stack. Requires
/// frame pointers; stops at the first implausible frame. Returns the number
/// of return addresses stored in `frames`, most recent call first.
#[inline(never)]
pub fn capture(frames: &mut [u64; MAX_FRAMES]) -> usize {
    let mut fp = current_frame_pointer();
    let mut sp = fp;
    let mut count = 0;
    while count < MAX_FRAMES {
        if fp == 0 || fp & 0x7 != 0 || fp < sp || fp - sp > 8 * 1024 * 1024 {
            break;
        }
        let (next_fp, return_address) = unsafe {
            let fp = fp as *const u64;
            (fp.read_volatile(), fp.add(1).read_volatile())
        };
        if return_address < 0x1000 {
            break;
        }
        frames[count] = return_address;
        count += 1;
        sp = fp;
        fp = next_fp;
    }
    count
}

#[cfg(target_arch = "x86_64")]
fn current_frame_pointer() -> u64 {
    let fp: u64;
    unsafe { core::arch::asm!("mov {}, rbp", out(reg) fp) };
    fp
}

#[cfg(target_arch = "aarch64")]
fn current_frame_pointer() -> u64 {
    let fp: u64;
    unsafe { core::arch::asm!("mov {}, x29", out(reg) fp) };
    fp
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn current_frame_pointer() -> u64 {
    0
}
