use std::collections::HashMap;

use lazy_static::lazy_static;
use pmu_data::EventDesc;

pub struct CPUFamily {
    pub name: String,
    pub vendor: String,
    pub id: String,
    pub events: HashMap<String, EventDesc>,
    pub aliases: HashMap<String, String>,
}

include!(concat!(env!("OUT_DIR"), "/events.rs"));

pub fn find_cpu_family(id: &str) -> Option<&CPUFamily> {
    CPU_FAMILIES.get(id)
}

#[cfg(target_arch = "x86_64")]
pub fn get_host_cpu_family() -> &'static str {
    const EAX_VENDOR_INFO: u32 = 0x1;

    let result = unsafe { core::arch::x86_64::__cpuid(EAX_VENDOR_INFO) };

    let eax = result.eax;

    let model = (eax >> 4) & 0xf;
    let family = (eax >> 8) & 0xf;
    let extended_model = (eax >> 16) & 0xf;
    let extended_family = (eax >> 20) & 0xff;

    if family == 0xf && extended_family == 0x8 {
        // AMD Family 23 (17h)
        if extended_model == 0x0 || extended_model == 0x1 || extended_model == 0x2 {
            return pmu_data::AMDZEN1;
        } else if extended_model == 0x3
            || extended_model == 0x4
            || extended_model == 0x6
            || extended_model == 0x7
            || extended_model == 0x9
        {
            return pmu_data::AMDZEN2;
        }
    } else if family == 0xf && extended_family == 0xa {
        // AMD Family 25 (19h)
        if extended_model == 0x0
            || extended_model == 0x2
            || extended_model == 0x4
            || extended_model == 0x5
        {
            return pmu_data::AMDZEN3;
        } else if extended_model == 0x1 || extended_model == 0x6 || extended_model == 0x7 {
            return pmu_data::AMDZEN4;
        }
    } else if family == 0x6 && extended_family == 0 {
        // Recent Intel processors
        if (extended_model == 0x3 && model == 0xc)
            || (extended_model == 0x4 && (model == 0x5 || model == 0x6))
        {
            return pmu_data::INTEL_HASWELL;
        } else if (extended_model == 0x3 && model == 0xd) || (extended_model == 0x4 && model == 0x7)
        {
            return pmu_data::INTEL_BROADWELL;
        } else if (extended_model == 0x5 && model == 0xe) || (extended_model == 0x4 && model == 0xe)
        {
            return pmu_data::INTEL_SKYLAKE;
        } else if (extended_model == 0x8 && model == 0xe) || (extended_model == 0x9 && model == 0xe)
        {
            return pmu_data::INTEL_KABYLAKE;
        } else if extended_model == 0xa && model == 0x5 {
            return pmu_data::INTEL_COMETLAKE;
        } else if extended_model == 0x7 && model == 0xe {
            return pmu_data::INTEL_ICELAKE;
        } else if extended_model == 0x6 && (model == 0xc || model == 0xa) {
            return pmu_data::INTEL_ICX;
        } else if extended_model == 0x8 && (model == 0xc || model == 0xd) {
            return pmu_data::INTEL_TIGERLAKE;
        } else if extended_model == 0xa && model == 0x7 {
            return pmu_data::INTEL_ROCKETLAKE;
        } else if extended_model == 0x9 && (model == 0x7 || model == 0xa) {
            return pmu_data::INTEL_ALDERLAKE;
        } else if extended_model == 0xb && (model == 0x7 || model == 0xa) {
            return pmu_data::INTEL_RAPTORLAKE;
        }
    }

    "unknown"
}

#[cfg(target_arch = "riscv64")]
pub fn get_host_cpu_family() -> &'static str {
    use proc_getter::cpuinfo::cpuinfo;

    let info = cpuinfo().expect("/proc/cpuinfo is inaccessible");
    let marchid = info[0].get("marchid");

    match marchid {
        Some(marchid) => {
            if marchid == "0x8000000000000007" {
                // TODO(Alex): technically speaking this also includes E7 and S7
                pmu_data::SIFIVE_U7
            } else {
                "unknown"
            }
        }
        None => "unknown",
    }
}
