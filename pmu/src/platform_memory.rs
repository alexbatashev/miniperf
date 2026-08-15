//! Linux system-scoped memory-controller bandwidth counters.

use std::io;

/// Cumulative memory-controller bytes since the monitor was enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryControllerSample {
    /// Bytes read by all discovered controllers.
    pub read_bytes: u64,
    /// Bytes written by all discovered controllers.
    pub write_bytes: u64,
    /// Package energy consumed while the monitor was enabled.
    pub package_joules: Option<f64>,
    /// CPU-core energy consumed while the monitor was enabled.
    pub core_joules: Option<f64>,
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use perf_event_open_sys::{bindings, bindings::perf_event_attr, ioctls};
    use std::{
        collections::HashMap,
        fs,
        os::fd::RawFd,
        path::{Path, PathBuf},
    };

    const READ_ALIASES: &[&str] = &[
        "cas_count_read",
        "dram_reads",
        "data_read",
        "data_reads",
        "read",
    ];
    const WRITE_ALIASES: &[&str] = &[
        "cas_count_write",
        "dram_writes",
        "data_write",
        "data_writes",
        "write",
    ];

    #[derive(Clone, Copy)]
    enum Measurement {
        ReadBytes,
        WriteBytes,
        PackageJoules,
        CoreJoules,
    }

    struct Handle {
        fd: RawFd,
        measurement: Measurement,
        scale: f64,
    }

    /// Active system-scoped controller counters.
    pub struct MemoryControllerMonitor {
        handles: Vec<Handle>,
    }

    impl MemoryControllerMonitor {
        /// Discover Intel IMC and AMD data-fabric PMUs. `None` means the host
        /// exposes no recognized controller events.
        pub fn start() -> io::Result<Option<Self>> {
            let root = Path::new("/sys/bus/event_source/devices");
            let mut handles = Vec::new();
            let mut last_error = None;
            for entry in match fs::read_dir(root) {
                Ok(value) => value,
                Err(_) => return Ok(None),
            } {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !(name.starts_with("uncore_imc") || name.starts_with("amd_df")) {
                    continue;
                }
                let path = entry.path();
                for (measurement, aliases) in [
                    (Measurement::ReadBytes, READ_ALIASES),
                    (Measurement::WriteBytes, WRITE_ALIASES),
                ] {
                    let Some((event_name, event_path)) = aliases.iter().find_map(|alias| {
                        let candidate = path.join("events").join(alias);
                        candidate
                            .is_file()
                            .then(|| ((*alias).to_string(), candidate))
                    }) else {
                        continue;
                    };
                    match open_event(&path, &event_name, &event_path, measurement) {
                        Ok(handle) => handles.push(handle),
                        Err(error) => {
                            // PMUs and aliases are independently exposed and
                            // independently permissioned. Preserve every
                            // usable channel instead of discarding a host's
                            // read bandwidth because one write event failed.
                            last_error = Some(error);
                        }
                    }
                }
            }
            if handles.is_empty() {
                return match last_error {
                    Some(error) => Err(error),
                    None => Ok(None),
                };
            }
            let power = root.join("power");
            for (measurement, event_name) in [
                (Measurement::PackageJoules, "energy-pkg"),
                (Measurement::CoreJoules, "energy-cores"),
            ] {
                let event_path = power.join("events").join(event_name);
                if !event_path.is_file() {
                    continue;
                }
                if let Ok(handle) = open_event(&power, event_name, &event_path, measurement) {
                    handles.push(handle);
                }
            }
            for handle in &handles {
                unsafe {
                    ioctls::RESET(handle.fd, 0);
                    ioctls::ENABLE(handle.fd, 0);
                }
            }
            Ok(Some(Self { handles }))
        }

        /// Read cumulative, multiplexing-scaled bytes.
        pub fn sample(&self) -> io::Result<MemoryControllerSample> {
            let mut result = MemoryControllerSample::default();
            for handle in &self.handles {
                let mut values = [0_u64; 3];
                let bytes = unsafe {
                    libc::read(
                        handle.fd,
                        values.as_mut_ptr().cast(),
                        std::mem::size_of_val(&values),
                    )
                };
                if bytes != std::mem::size_of_val(&values) as isize {
                    return Err(io::Error::last_os_error());
                }
                let scaled = if values[2] == 0 {
                    0.0
                } else {
                    values[0] as f64 * values[1] as f64 / values[2] as f64
                };
                let value = (scaled * handle.scale).max(0.0);
                match handle.measurement {
                    Measurement::ReadBytes => {
                        result.read_bytes = result
                            .read_bytes
                            .saturating_add(value.min(u64::MAX as f64) as u64)
                    }
                    Measurement::WriteBytes => {
                        result.write_bytes = result
                            .write_bytes
                            .saturating_add(value.min(u64::MAX as f64) as u64)
                    }
                    Measurement::PackageJoules => {
                        result.package_joules = Some(result.package_joules.unwrap_or(0.0) + value)
                    }
                    Measurement::CoreJoules => {
                        result.core_joules = Some(result.core_joules.unwrap_or(0.0) + value)
                    }
                }
            }
            Ok(result)
        }
    }

    impl Drop for MemoryControllerMonitor {
        fn drop(&mut self) {
            for handle in &self.handles {
                unsafe {
                    ioctls::DISABLE(handle.fd, 0);
                    libc::close(handle.fd);
                }
            }
        }
    }

    fn open_event(
        device: &Path,
        event_name: &str,
        event_path: &Path,
        measurement: Measurement,
    ) -> io::Result<Handle> {
        let pmu_type = read_number(&device.join("type"))? as u32;
        let cpu = fs::read_to_string(device.join("cpumask"))
            .or_else(|_| fs::read_to_string(device.join("cpus")))
            .ok()
            .and_then(|value| first_cpu(&value))
            .unwrap_or(0) as i32;
        let formats = read_formats(&device.join("format"))?;
        let assignments = fs::read_to_string(event_path)?;
        let mut registers = [0_u64; 3];
        for assignment in assignments.trim().split(',') {
            let Some((name, value)) = assignment.split_once('=') else {
                continue;
            };
            let value = parse_number(value.trim())?;
            if let Some((register, bits)) = formats.get(name.trim()) {
                apply_bits(&mut registers[*register], bits, value);
            }
        }
        let mut attr: perf_event_attr = unsafe { std::mem::zeroed() };
        attr.size = std::mem::size_of::<perf_event_attr>() as u32;
        attr.type_ = pmu_type;
        attr.config = registers[0];
        attr.config1 = registers[1];
        attr.config2 = registers[2];
        attr.set_disabled(1);
        attr.read_format = (bindings::PERF_FORMAT_TOTAL_TIME_ENABLED
            | bindings::PERF_FORMAT_TOTAL_TIME_RUNNING) as u64;
        let fd = unsafe { perf_event_open_sys::perf_event_open(&mut attr, -1, cpu, -1, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle {
            fd,
            measurement,
            scale: event_scale(device, event_name).unwrap_or(match measurement {
                Measurement::ReadBytes | Measurement::WriteBytes => 64.0,
                Measurement::PackageJoules | Measurement::CoreJoules => 1.0,
            }),
        })
    }

    fn event_scale(device: &Path, event: &str) -> Option<f64> {
        let scale = fs::read_to_string(device.join("events").join(format!("{event}.scale")))
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()?;
        let unit = fs::read_to_string(device.join("events").join(format!("{event}.unit")))
            .unwrap_or_default()
            .to_ascii_lowercase();
        Some(
            scale
                * if unit.contains("mib") {
                    1024.0 * 1024.0
                } else if unit.contains("kib") {
                    1024.0
                } else {
                    1.0
                },
        )
    }

    fn read_formats(path: &Path) -> io::Result<HashMap<String, (usize, Vec<u32>)>> {
        let mut result = HashMap::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let value = fs::read_to_string(entry.path())?;
            let (register, bits) = value
                .trim()
                .split_once(':')
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid PMU format"))?;
            let register = match register {
                "config" => 0,
                "config1" => 1,
                "config2" => 2,
                _ => continue,
            };
            result.insert(
                entry.file_name().to_string_lossy().into_owned(),
                (register, parse_bits(bits)?),
            );
        }
        Ok(result)
    }

    fn parse_bits(value: &str) -> io::Result<Vec<u32>> {
        let mut bits = Vec::new();
        for part in value.split(',') {
            if let Some((start, end)) = part.split_once('-') {
                bits.extend(
                    start.parse::<u32>().map_err(invalid)?.to_owned()
                        ..=end.parse::<u32>().map_err(invalid)?,
                );
            } else {
                bits.push(part.parse::<u32>().map_err(invalid)?);
            }
        }
        Ok(bits)
    }

    fn apply_bits(target: &mut u64, bits: &[u32], value: u64) {
        for (source, destination) in bits.iter().enumerate() {
            if value & (1_u64 << source) != 0 {
                *target |= 1_u64 << destination;
            }
        }
    }
    fn first_cpu(value: &str) -> Option<u32> {
        value
            .trim()
            .split(',')
            .next()?
            .split('-')
            .next()?
            .parse()
            .ok()
    }
    fn read_number(path: &PathBuf) -> io::Result<u64> {
        parse_number(fs::read_to_string(path)?.trim())
    }
    fn parse_number(value: &str) -> io::Result<u64> {
        u64::from_str_radix(
            value.trim_start_matches("0x"),
            if value.starts_with("0x") { 16 } else { 10 },
        )
        .map_err(invalid)
    }
    fn invalid(error: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, error.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_and_applies_sparse_sysfs_format_bits() {
            let bits = parse_bits("0-3,8-9").unwrap();
            assert_eq!(bits, vec![0, 1, 2, 3, 8, 9]);
            let mut target = 0;
            apply_bits(&mut target, &bits, 0b11_1010);
            assert_eq!(target, 0b11_0000_1010);
        }

        #[test]
        fn parses_first_cpu_from_range_list() {
            assert_eq!(first_cpu("4-7,12-15\n"), Some(4));
        }

        #[test]
        fn recognizes_intel_free_running_imc_event_names() {
            assert!(READ_ALIASES.contains(&"data_read"));
            assert!(WRITE_ALIASES.contains(&"data_write"));
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::MemoryControllerMonitor;

#[cfg(not(target_os = "linux"))]
/// Unsupported-platform stub.
pub struct MemoryControllerMonitor;

#[cfg(not(target_os = "linux"))]
impl MemoryControllerMonitor {
    /// No platform memory counters are available.
    pub fn start() -> io::Result<Option<Self>> {
        Ok(None)
    }
    /// Return an empty sample.
    pub fn sample(&self) -> io::Result<MemoryControllerSample> {
        Ok(MemoryControllerSample::default())
    }
}
