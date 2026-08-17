use serde::{Deserialize, Serialize};

use crate::UserRegs;

/// The kind of memory access a precise sample describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemOp {
    Load,
    Store,
    Prefetch,
    Exec,
    Unknown,
}

/// Where the access was satisfied, normalized across PEBS, IBS and SPE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemLevel {
    L1,
    Lfb,
    L2,
    L3,
    L4,
    Ram,
    Pmem,
    Cxl,
    Io,
    Uncached,
    Unknown,
}

/// Coherence response observed for the access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemSnoop {
    None,
    Hit,
    Miss,
    /// Another core held the line modified: the false-sharing signal.
    Hitm,
    Fwd,
    Peer,
    Unknown,
}

/// Address translation outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemTlb {
    Hit,
    Miss,
    Unknown,
}

/// One precise memory sample's data source, normalized so that PEBS, IBS and
/// SPE decoders all produce the same shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemDataSource {
    pub op: MemOp,
    pub level: MemLevel,
    /// `Some(true)` hit, `Some(false)` miss, `None` when the hardware did not say.
    pub hit: Option<bool>,
    pub snoop: MemSnoop,
    /// Serviced by a remote NUMA node.
    pub remote: bool,
    pub tlb: MemTlb,
    /// Access carried a bus lock.
    pub locked: bool,
}

const PERF_MEM_OP_SHIFT: u32 = 0;
const PERF_MEM_LVL_SHIFT: u32 = 5;
const PERF_MEM_SNOOP_SHIFT: u32 = 19;
const PERF_MEM_LOCK_SHIFT: u32 = 24;
const PERF_MEM_TLB_SHIFT: u32 = 26;
const PERF_MEM_LVLNUM_SHIFT: u32 = 33;
const PERF_MEM_REMOTE_SHIFT: u32 = 37;
const PERF_MEM_SNOOPX_SHIFT: u32 = 38;

impl MemDataSource {
    /// Decode the `PERF_SAMPLE_DATA_SRC` union laid out by `perf_event.h`.
    pub fn from_perf(data_src: u64) -> MemDataSource {
        let field = |shift: u32, bits: u32| (data_src >> shift) & ((1 << bits) - 1);
        let op = field(PERF_MEM_OP_SHIFT, 5);
        let lvl = field(PERF_MEM_LVL_SHIFT, 14);
        let snoop = field(PERF_MEM_SNOOP_SHIFT, 5);
        let lock = field(PERF_MEM_LOCK_SHIFT, 2);
        let tlb = field(PERF_MEM_TLB_SHIFT, 7);
        let lvl_num = field(PERF_MEM_LVLNUM_SHIFT, 4);
        let remote = field(PERF_MEM_REMOTE_SHIFT, 1);
        let snoopx = field(PERF_MEM_SNOOPX_SHIFT, 2);

        MemDataSource {
            op: match op {
                _ if op & 0x02 != 0 => MemOp::Load,
                _ if op & 0x04 != 0 => MemOp::Store,
                _ if op & 0x08 != 0 => MemOp::Prefetch,
                _ if op & 0x10 != 0 => MemOp::Exec,
                _ => MemOp::Unknown,
            },
            level: level_of(lvl_num, lvl),
            hit: match (lvl & 0x02 != 0, lvl & 0x04 != 0) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            },
            snoop: match () {
                _ if snoop & 0x10 != 0 => MemSnoop::Hitm,
                _ if snoopx & 0x02 != 0 => MemSnoop::Peer,
                _ if snoopx & 0x01 != 0 => MemSnoop::Fwd,
                _ if snoop & 0x04 != 0 => MemSnoop::Hit,
                _ if snoop & 0x08 != 0 => MemSnoop::Miss,
                _ if snoop & 0x02 != 0 => MemSnoop::None,
                _ => MemSnoop::Unknown,
            },
            remote: remote != 0,
            tlb: match () {
                _ if tlb & 0x04 != 0 => MemTlb::Miss,
                _ if tlb & 0x02 != 0 => MemTlb::Hit,
                _ => MemTlb::Unknown,
            },
            locked: lock & 0x02 != 0,
        }
    }
}

/// `mem_lvl_num` is authoritative on modern kernels; the deprecated `mem_lvl`
/// bitmask is the fallback for hardware and kernels that only fill it in.
fn level_of(lvl_num: u64, lvl: u64) -> MemLevel {
    match lvl_num {
        0x01 => return MemLevel::L1,
        0x02 => return MemLevel::L2,
        0x03 => return MemLevel::L3,
        0x04 => return MemLevel::L4,
        0x08 => return MemLevel::Uncached,
        0x09 => return MemLevel::Cxl,
        0x0a => return MemLevel::Io,
        0x0c => return MemLevel::Lfb,
        0x0d => return MemLevel::Ram,
        0x0e => return MemLevel::Pmem,
        _ => {}
    }
    match () {
        _ if lvl & 0x0008 != 0 => MemLevel::L1,
        _ if lvl & 0x0010 != 0 => MemLevel::Lfb,
        _ if lvl & 0x0020 != 0 => MemLevel::L2,
        _ if lvl & 0x0040 != 0 => MemLevel::L3,
        _ if lvl & 0x0380 != 0 => MemLevel::Ram,
        _ if lvl & 0x0c00 != 0 => MemLevel::L3,
        _ if lvl & 0x1000 != 0 => MemLevel::Io,
        _ if lvl & 0x2000 != 0 => MemLevel::Uncached,
        _ => MemLevel::Unknown,
    }
}

impl MemOp {
    pub fn as_str(self) -> &'static str {
        match self {
            MemOp::Load => "load",
            MemOp::Store => "store",
            MemOp::Prefetch => "prefetch",
            MemOp::Exec => "exec",
            MemOp::Unknown => "unknown",
        }
    }
}

impl MemLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            MemLevel::L1 => "l1",
            MemLevel::Lfb => "lfb",
            MemLevel::L2 => "l2",
            MemLevel::L3 => "l3",
            MemLevel::L4 => "l4",
            MemLevel::Ram => "ram",
            MemLevel::Pmem => "pmem",
            MemLevel::Cxl => "cxl",
            MemLevel::Io => "io",
            MemLevel::Uncached => "uncached",
            MemLevel::Unknown => "unknown",
        }
    }
}

impl MemSnoop {
    pub fn as_str(self) -> &'static str {
        match self {
            MemSnoop::None => "none",
            MemSnoop::Hit => "hit",
            MemSnoop::Miss => "miss",
            MemSnoop::Hitm => "hitm",
            MemSnoop::Fwd => "fwd",
            MemSnoop::Peer => "peer",
            MemSnoop::Unknown => "unknown",
        }
    }
}

impl MemTlb {
    pub fn as_str(self) -> &'static str {
        match self {
            MemTlb::Hit => "hit",
            MemTlb::Miss => "miss",
            MemTlb::Unknown => "unknown",
        }
    }
}

/// One precise memory sample as recorded, before symbolization.
#[derive(Clone, Debug)]
pub struct MemSample {
    pub timestamp: u64,
    pub process_id: u32,
    pub thread_id: u32,
    pub cpu: u32,
    /// Virtual address the access targeted.
    pub data_addr: u64,
    /// Access latency in core cycles, zero when the hardware did not report it.
    pub latency: u64,
    /// Raw vendor data-source encoding; decoded by [`MemDataSource::from_perf`].
    pub data_src: u64,
    /// Instruction pointer first, then the kernel-provided callchain.
    pub callstack: Vec<u64>,
    /// Call stack reconstructed from hardware branch records. Empty when the
    /// host has no branch recorder.
    pub lbr_callstack: Vec<u64>,
    pub user_regs: Option<UserRegs>,
    pub user_stack: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `data_src` the way `perf_event.h` does, from hand-picked fields.
    fn encode(
        op: u64,
        lvl: u64,
        lvl_num: u64,
        snoop: u64,
        tlb: u64,
        lock: u64,
        remote: u64,
    ) -> u64 {
        op | (lvl << PERF_MEM_LVL_SHIFT)
            | (snoop << PERF_MEM_SNOOP_SHIFT)
            | (lock << PERF_MEM_LOCK_SHIFT)
            | (tlb << PERF_MEM_TLB_SHIFT)
            | (lvl_num << PERF_MEM_LVLNUM_SHIFT)
            | (remote << PERF_MEM_REMOTE_SHIFT)
    }

    #[test]
    fn decodes_an_l1_load_hit() {
        // op=LOAD, lvl=HIT|L1, lvl_num=L1, snoop=NONE, tlb=HIT|L1, lock=NA
        let decoded =
            MemDataSource::from_perf(encode(0x02, 0x02 | 0x08, 0x01, 0x02, 0x02 | 0x08, 0x01, 0));
        assert_eq!(decoded.op, MemOp::Load);
        assert_eq!(decoded.level, MemLevel::L1);
        assert_eq!(decoded.hit, Some(true));
        assert_eq!(decoded.snoop, MemSnoop::None);
        assert_eq!(decoded.tlb, MemTlb::Hit);
        assert!(!decoded.locked);
        assert!(!decoded.remote);
    }

    #[test]
    fn decodes_a_remote_hitm_dram_load() {
        // op=LOAD, lvl=MISS, lvl_num=RAM, snoop=HITM, tlb=MISS, lock=LOCKED, remote
        let decoded = MemDataSource::from_perf(encode(0x02, 0x04, 0x0d, 0x10, 0x04, 0x02, 0x01));
        assert_eq!(decoded.level, MemLevel::Ram);
        assert_eq!(decoded.hit, Some(false));
        assert_eq!(decoded.snoop, MemSnoop::Hitm);
        assert_eq!(decoded.tlb, MemTlb::Miss);
        assert!(decoded.locked);
        assert!(decoded.remote);
    }

    #[test]
    fn falls_back_to_the_deprecated_level_bitmask() {
        // lvl_num=NA, lvl=HIT|L3, op=STORE
        let decoded = MemDataSource::from_perf(encode(0x04, 0x02 | 0x40, 0x0f, 0, 0, 0, 0));
        assert_eq!(decoded.op, MemOp::Store);
        assert_eq!(decoded.level, MemLevel::L3);
        assert_eq!(decoded.hit, Some(true));
    }

    #[test]
    fn an_empty_encoding_is_entirely_unknown() {
        let decoded = MemDataSource::from_perf(0);
        assert_eq!(decoded.op, MemOp::Unknown);
        assert_eq!(decoded.level, MemLevel::Unknown);
        assert_eq!(decoded.hit, None);
        assert_eq!(decoded.snoop, MemSnoop::Unknown);
        assert_eq!(decoded.tlb, MemTlb::Unknown);
    }
}
