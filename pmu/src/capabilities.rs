/// Host facilities that influence which profiling features are available.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Linux `perf_event_paranoid`, when readable.
    pub perf_event_paranoid: Option<i32>,
    /// Whether the current process has `CAP_PERFMON` in its effective set.
    pub has_cap_perfmon: bool,
    /// Whether a hardware CPU-cycles event can be opened for the current thread.
    pub hardware_counters: bool,
    /// Kernel limit for frequency-mode sampling.
    pub max_sample_rate: Option<u64>,
    /// Whether kernel symbol addresses are available to this process.
    pub kernel_symbols: bool,
    /// Per-CPU perf mmap allowance in KiB (`perf_event_mlock_kb`).
    pub mmap_limit_kb: Option<u64>,
    /// Whether the kernel accepts perf's mmap data-watermark wakeup mode.
    pub mmap_data_watermark: bool,
}

/// Probe profiling-related kernel capabilities without panicking.
pub fn capabilities() -> Capabilities {
    #[cfg(target_os = "linux")]
    {
        linux_capabilities()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Capabilities::default()
    }
}

#[cfg(target_os = "linux")]
fn linux_capabilities() -> Capabilities {
    capabilities_from_probe(ProbeResult {
        perf_event_paranoid: read_number("/proc/sys/kernel/perf_event_paranoid"),
        effective_capabilities: effective_capabilities(),
        hardware_counters: probe_hardware_counter(),
        max_sample_rate: read_number("/proc/sys/kernel/perf_event_max_sample_rate"),
        kptr_restrict: read_number("/proc/sys/kernel/kptr_restrict"),
        mmap_limit_kb: read_number("/proc/sys/kernel/perf_event_mlock_kb"),
        mmap_data_watermark: probe_mmap_data_watermark(),
        effective_uid: unsafe { libc::geteuid() },
    })
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct ProbeResult {
    perf_event_paranoid: Option<i32>,
    effective_capabilities: Option<u64>,
    hardware_counters: bool,
    max_sample_rate: Option<u64>,
    kptr_restrict: Option<u32>,
    mmap_limit_kb: Option<u64>,
    mmap_data_watermark: bool,
    effective_uid: u32,
}

#[cfg(target_os = "linux")]
fn capabilities_from_probe(probe: ProbeResult) -> Capabilities {
    let has_cap_perfmon = probe
        .effective_capabilities
        .is_some_and(|caps| caps & (1_u64 << 38) != 0);
    let kernel_symbols = matches!(probe.kptr_restrict, Some(0))
        || (matches!(probe.kptr_restrict, Some(1))
            && (has_cap_perfmon || probe.effective_uid == 0));

    Capabilities {
        perf_event_paranoid: probe.perf_event_paranoid,
        has_cap_perfmon,
        hardware_counters: probe.hardware_counters,
        max_sample_rate: probe.max_sample_rate,
        kernel_symbols,
        mmap_limit_kb: probe.mmap_limit_kb,
        mmap_data_watermark: probe.mmap_data_watermark,
    }
}

#[cfg(target_os = "linux")]
fn read_number<T: std::str::FromStr>(path: &str) -> Option<T> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn effective_capabilities() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let raw = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))?
        .trim();
    u64::from_str_radix(raw, 16).ok()
}

#[cfg(target_os = "linux")]
fn probe_hardware_counter() -> bool {
    use perf_event_open_sys::{bindings, bindings::perf_event_attr};

    let mut attr = perf_event_attr::default();
    attr.size = std::mem::size_of::<perf_event_attr>() as u32;
    attr.type_ = bindings::PERF_TYPE_HARDWARE;
    attr.config = bindings::PERF_COUNT_HW_CPU_CYCLES as u64;
    attr.set_disabled(1);
    attr.set_exclude_kernel(1);
    attr.set_exclude_hv(1);
    let fd = unsafe { perf_event_open_sys::perf_event_open(&mut attr, 0, -1, -1, 0) };
    if fd < 0 {
        false
    } else {
        unsafe { libc::close(fd) };
        true
    }
}

#[cfg(target_os = "linux")]
fn probe_mmap_data_watermark() -> bool {
    use perf_event_open_sys::{bindings, bindings::perf_event_attr};

    // Opening a disabled software event makes the kernel validate the attr
    // without requiring a hardware PMU or creating an mmap. A successful open
    // proves that watermark-mode wakeups are usable by this process/kernel.
    let mut attr = perf_event_attr::default();
    attr.size = std::mem::size_of::<perf_event_attr>() as u32;
    attr.type_ = bindings::PERF_TYPE_SOFTWARE;
    attr.config = bindings::PERF_COUNT_SW_DUMMY as u64;
    attr.set_disabled(1);
    attr.set_exclude_kernel(1);
    attr.set_exclude_hv(1);
    attr.set_watermark(1);
    attr.wakeup_watermark = 1;
    let fd = unsafe { perf_event_open_sys::perf_event_open(&mut attr, 0, -1, -1, 0) };
    if fd < 0 {
        false
    } else {
        unsafe { libc::close(fd) };
        true
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn probe_is_total() {
        let caps = capabilities();
        assert!(caps.max_sample_rate.is_some());
        assert!(caps.mmap_limit_kb.is_some());
    }

    #[test]
    fn maps_mocked_probe_results() {
        let caps = capabilities_from_probe(ProbeResult {
            perf_event_paranoid: Some(4),
            effective_capabilities: Some(1_u64 << 38),
            hardware_counters: false,
            max_sample_rate: Some(12_345),
            kptr_restrict: Some(1),
            mmap_limit_kb: Some(516),
            mmap_data_watermark: true,
            effective_uid: 1000,
        });

        assert_eq!(caps.perf_event_paranoid, Some(4));
        assert!(caps.has_cap_perfmon);
        assert!(!caps.hardware_counters);
        assert_eq!(caps.max_sample_rate, Some(12_345));
        assert!(caps.kernel_symbols);
        assert_eq!(caps.mmap_limit_kb, Some(516));
        assert!(caps.mmap_data_watermark);
    }

    #[test]
    fn mocked_probe_reports_unavailable_watermark_without_panicking() {
        let caps = capabilities_from_probe(ProbeResult {
            perf_event_paranoid: Some(4),
            hardware_counters: false,
            mmap_data_watermark: false,
            ..ProbeResult::default()
        });

        assert_eq!(caps.perf_event_paranoid, Some(4));
        assert!(!caps.hardware_counters);
        assert!(!caps.mmap_data_watermark);
    }
}
