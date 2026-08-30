//! Smoke-test the Arm SPE sampling driver against a live workload:
//! `cargo run --release --example spe_probe [-- <cpu>]` (needs root or
//! perf_event_paranoid <= 0 on most hosts).

#[cfg(target_os = "linux")]
fn main() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let cpu = std::env::args().nth(1).unwrap_or_else(|| "6".to_string());
    let command: Vec<String> = std::env::args().skip(2).collect();
    let command = if command.is_empty() {
        vec!["sha256sum".to_string(), "/dev/zero".to_string()]
    } else {
        command
    };
    let mut child = std::process::Command::new("taskset")
        .arg("-c")
        .arg(&cpu)
        .args(&command)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn workload");

    let mut driver = match libprof::mem_sampling_driver(child.id() as i32, 1000, 0, false) {
        Ok(driver) => driver,
        Err(error) => {
            let _ = child.kill();
            eprintln!("driver open failed: {error}");
            std::process::exit(1);
        }
    };

    let samples = Arc::new(AtomicU64::new(0));
    let printed = Arc::new(AtomicU64::new(0));
    let counter = samples.clone();
    driver
        .start(Arc::new(move |record| {
            if let libprof::Record::MemSample(sample) = record {
                counter.fetch_add(1, Ordering::Relaxed);
                if printed.fetch_add(1, Ordering::Relaxed) < 10 {
                    println!(
                        "cpu={} ip={:#x} addr={:#x} lat={} data_src={:#x} time={}",
                        sample.cpu,
                        sample.ip,
                        sample.data_addr,
                        sample.latency,
                        sample.data_src,
                        sample.time
                    );
                }
            }
        }))
        .expect("start driver");

    std::thread::sleep(std::time::Duration::from_secs(3));
    let stop = driver.stop();
    let _ = child.kill();
    let _ = child.wait();
    println!(
        "total samples: {} (stop: {:?})",
        samples.load(Ordering::Relaxed),
        stop.err().map(|error| error.to_string())
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("linux only");
}
