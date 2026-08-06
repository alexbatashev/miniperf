use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use mperf_data::RooflineCalibration;
use rayon::prelude::*;

const SAMPLES: usize = 5;
const COMPUTE_ITERATIONS: usize = 20_000_000;
const MEMORY_ELEMENTS: usize = 8 * 1024 * 1024;
const MEMORY_REPETITIONS: usize = 8;
const MEMORY_CHUNK_ELEMENTS: usize = 16 * 1024;

pub(super) fn measure() -> Result<RooflineCalibration> {
    let threads = rayon::current_num_threads();
    let kernel = compute_kernel();

    // Populate the Rayon pool and architecture state before timed samples.
    let _ = measure_compute_sample(kernel, threads, COMPUTE_ITERATIONS / 100);

    let compute_samples = (0..SAMPLES)
        .map(|_| measure_compute_sample(kernel, threads, COMPUTE_ITERATIONS))
        .collect::<Vec<_>>();

    let mut a = allocate_vector(MEMORY_ELEMENTS).context("allocate roofline triad input A")?;
    let mut b = allocate_vector(MEMORY_ELEMENTS).context("allocate roofline triad input B")?;
    let mut c = allocate_vector(MEMORY_ELEMENTS).context("allocate roofline triad output")?;
    a.par_iter_mut()
        .enumerate()
        .for_each(|(index, value)| *value = 1.0 + (index % 17) as f64 * 1.0e-6);
    b.par_iter_mut()
        .enumerate()
        .for_each(|(index, value)| *value = 2.0 + (index % 31) as f64 * 1.0e-6);
    c.par_iter_mut().for_each(|value| *value = 0.0);

    let _ = measure_memory_sample(&a, &b, &mut c, 1);
    let memory_samples = (0..SAMPLES)
        .map(|_| measure_memory_sample(&a, &b, &mut c, MEMORY_REPETITIONS))
        .collect::<Vec<_>>();

    let fp64_gflops = median(&compute_samples);
    let memory_gbytes_per_second = median(&memory_samples);
    if !fp64_gflops.is_finite() || fp64_gflops <= 0.0 {
        bail!("FP64 roof calibration produced an invalid result: {fp64_gflops}");
    }
    if !memory_gbytes_per_second.is_finite() || memory_gbytes_per_second <= 0.0 {
        bail!(
            "memory roof calibration produced an invalid result: \
             {memory_gbytes_per_second}"
        );
    }
    let memory_working_set_bytes = 3_u64
        .saturating_mul(MEMORY_ELEMENTS as u64)
        .saturating_mul(size_of::<f64>() as u64);

    Ok(RooflineCalibration {
        threads,
        cpu_affinity: cpu_affinity(),
        samples: SAMPLES,
        compute_kernel: kernel.name.to_string(),
        fp64_gflops,
        fp64_gflops_samples: compute_samples,
        memory_gbytes_per_second,
        memory_gbytes_per_second_samples: memory_samples,
        ridge_point_flops_per_byte: fp64_gflops / memory_gbytes_per_second,
        memory_working_set_bytes,
    })
}

fn allocate_vector(elements: usize) -> Result<Vec<f64>> {
    let mut vector = Vec::new();
    vector
        .try_reserve_exact(elements)
        .context("reserve calibration memory")?;
    vector.resize(elements, 0.0);
    Ok(vector)
}

#[derive(Clone, Copy)]
struct ComputeKernel {
    name: &'static str,
    flops_per_iteration: u64,
    run: unsafe fn(usize, usize) -> f64,
}

fn compute_kernel() -> ComputeKernel {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("fma")
        {
            return ComputeKernel {
                name: "x86-avx512-fma-f64",
                flops_per_iteration: 128,
                run: compute_avx512,
            };
        }
        if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
        {
            return ComputeKernel {
                name: "x86-avx2-fma-f64",
                flops_per_iteration: 64,
                run: compute_avx2,
            };
        }
    }

    #[cfg(all(target_arch = "riscv64", target_os = "linux"))]
    if rvv_available() {
        let vl = unsafe { mperf_roofline_rvv_vlmax() };
        if vl > 0 {
            return ComputeKernel {
                name: "riscv-rvv-fma-f64",
                flops_per_iteration: 16 * vl as u64,
                run: compute_rvv,
            };
        }
    }

    ComputeKernel {
        name: "scalar-fma-f64",
        flops_per_iteration: 16,
        run: compute_scalar,
    }
}

#[cfg(all(target_arch = "riscv64", target_os = "linux"))]
fn rvv_available() -> bool {
    const RISCV_HWCAP_ISA_V: libc::c_ulong = 1 << (b'V' - b'A');
    unsafe { libc::getauxval(libc::AT_HWCAP) & RISCV_HWCAP_ISA_V != 0 }
}

#[cfg(target_arch = "riscv64")]
unsafe extern "C" {
    fn mperf_roofline_rvv_vlmax() -> usize;
    fn mperf_roofline_compute_rvv(iterations: usize, worker: usize) -> f64;
}

#[cfg(target_arch = "riscv64")]
unsafe fn compute_rvv(iterations: usize, worker: usize) -> f64 {
    unsafe { mperf_roofline_compute_rvv(iterations, worker) }
}

fn measure_compute_sample(kernel: ComputeKernel, threads: usize, iterations: usize) -> f64 {
    let start = Instant::now();
    let checksum = rayon::broadcast(|context| unsafe { (kernel.run)(iterations, context.index()) })
        .into_iter()
        .sum::<f64>();
    let elapsed = nonzero_elapsed(start.elapsed());
    black_box(checksum);

    iterations as f64 * kernel.flops_per_iteration as f64 * threads as f64
        / elapsed.as_secs_f64()
        / 1.0e9
}

fn measure_memory_sample(a: &[f64], b: &[f64], c: &mut [f64], repetitions: usize) -> f64 {
    let start = Instant::now();
    for repetition in 0..repetitions {
        c.par_chunks_mut(MEMORY_CHUNK_ELEMENTS)
            .enumerate()
            .for_each(|(chunk_index, output)| {
                let offset = chunk_index * MEMORY_CHUNK_ELEMENTS;
                let a = &a[offset..offset + output.len()];
                let b = &b[offset..offset + output.len()];
                for index in 0..output.len() {
                    output[index] = b[index].mul_add(1.000_000_1, a[index]);
                }
            });
        black_box(c[(repetition * 8191) % c.len()]);
    }
    let elapsed = nonzero_elapsed(start.elapsed());
    let bytes = repetitions as f64 * c.len() as f64 * 3.0 * size_of::<f64>() as f64;
    bytes / elapsed.as_secs_f64() / 1.0e9
}

fn nonzero_elapsed(elapsed: Duration) -> Duration {
    elapsed.max(Duration::from_nanos(1))
}

fn median(values: &[f64]) -> f64 {
    let mut values = values.to_vec();
    values.sort_unstable_by(f64::total_cmp);
    values[values.len() / 2]
}

#[cfg(target_os = "linux")]
fn cpu_affinity() -> Option<String> {
    let mut set = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
    if unsafe { libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut set) } != 0 {
        return None;
    }
    let cpus = (0..libc::CPU_SETSIZE as usize)
        .filter(|cpu| unsafe { libc::CPU_ISSET(*cpu, &set) })
        .collect::<Vec<_>>();
    (!cpus.is_empty()).then(|| format_cpu_list(&cpus))
}

#[cfg(not(target_os = "linux"))]
fn cpu_affinity() -> Option<String> {
    None
}

#[cfg(any(target_os = "linux", test))]
fn format_cpu_list(cpus: &[usize]) -> String {
    let mut ranges = Vec::new();
    let mut start = *cpus.first().unwrap_or(&0);
    let mut end = start;
    for &cpu in cpus.iter().skip(1) {
        if cpu == end + 1 {
            end = cpu;
        } else {
            ranges.push((start, end));
            start = cpu;
            end = cpu;
        }
    }
    if !cpus.is_empty() {
        ranges.push((start, end));
    }
    ranges
        .into_iter()
        .map(|(start, end)| {
            if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

unsafe fn compute_scalar(iterations: usize, worker: usize) -> f64 {
    let seed = 0.1 * (worker + 1) as f64;
    let multiplier = 1.000_000_000_000_000_2_f64;
    let addend = 0.000_000_000_000_000_1_f64;
    let mut accumulators = std::array::from_fn::<_, 8, _>(|index| seed + index as f64 * 0.1);
    for _ in 0..iterations {
        for accumulator in &mut accumulators {
            *accumulator = accumulator.mul_add(multiplier, addend);
        }
    }
    black_box(accumulators).into_iter().sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn compute_avx2(iterations: usize, worker: usize) -> f64 {
    use std::arch::x86_64::*;

    let seed = _mm256_set1_pd(0.1 * (worker + 1) as f64);
    let multiplier = _mm256_set1_pd(1.000_000_000_000_000_2);
    let addend = _mm256_set1_pd(0.000_000_000_000_000_1);
    let increment = _mm256_set1_pd(0.1);
    let mut a0 = seed;
    let mut a1 = _mm256_add_pd(a0, increment);
    let mut a2 = _mm256_add_pd(a1, increment);
    let mut a3 = _mm256_add_pd(a2, increment);
    let mut a4 = _mm256_add_pd(a3, increment);
    let mut a5 = _mm256_add_pd(a4, increment);
    let mut a6 = _mm256_add_pd(a5, increment);
    let mut a7 = _mm256_add_pd(a6, increment);

    for _ in 0..iterations {
        a0 = _mm256_fmadd_pd(a0, multiplier, addend);
        a1 = _mm256_fmadd_pd(a1, multiplier, addend);
        a2 = _mm256_fmadd_pd(a2, multiplier, addend);
        a3 = _mm256_fmadd_pd(a3, multiplier, addend);
        a4 = _mm256_fmadd_pd(a4, multiplier, addend);
        a5 = _mm256_fmadd_pd(a5, multiplier, addend);
        a6 = _mm256_fmadd_pd(a6, multiplier, addend);
        a7 = _mm256_fmadd_pd(a7, multiplier, addend);
    }

    let sum01 = _mm256_add_pd(a0, a1);
    let sum23 = _mm256_add_pd(a2, a3);
    let sum45 = _mm256_add_pd(a4, a5);
    let sum67 = _mm256_add_pd(a6, a7);
    let sum = _mm256_add_pd(_mm256_add_pd(sum01, sum23), _mm256_add_pd(sum45, sum67));
    let mut lanes = [0.0; 4];
    _mm256_storeu_pd(lanes.as_mut_ptr(), sum);
    black_box(lanes).into_iter().sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,fma")]
unsafe fn compute_avx512(iterations: usize, worker: usize) -> f64 {
    use std::arch::x86_64::*;

    let seed = _mm512_set1_pd(0.1 * (worker + 1) as f64);
    let multiplier = _mm512_set1_pd(1.000_000_000_000_000_2);
    let addend = _mm512_set1_pd(0.000_000_000_000_000_1);
    let increment = _mm512_set1_pd(0.1);
    let mut a0 = seed;
    let mut a1 = _mm512_add_pd(a0, increment);
    let mut a2 = _mm512_add_pd(a1, increment);
    let mut a3 = _mm512_add_pd(a2, increment);
    let mut a4 = _mm512_add_pd(a3, increment);
    let mut a5 = _mm512_add_pd(a4, increment);
    let mut a6 = _mm512_add_pd(a5, increment);
    let mut a7 = _mm512_add_pd(a6, increment);

    for _ in 0..iterations {
        a0 = _mm512_fmadd_pd(a0, multiplier, addend);
        a1 = _mm512_fmadd_pd(a1, multiplier, addend);
        a2 = _mm512_fmadd_pd(a2, multiplier, addend);
        a3 = _mm512_fmadd_pd(a3, multiplier, addend);
        a4 = _mm512_fmadd_pd(a4, multiplier, addend);
        a5 = _mm512_fmadd_pd(a5, multiplier, addend);
        a6 = _mm512_fmadd_pd(a6, multiplier, addend);
        a7 = _mm512_fmadd_pd(a7, multiplier, addend);
    }

    let sum01 = _mm512_add_pd(a0, a1);
    let sum23 = _mm512_add_pd(a2, a3);
    let sum45 = _mm512_add_pd(a4, a5);
    let sum67 = _mm512_add_pd(a6, a7);
    let sum = _mm512_add_pd(_mm512_add_pd(sum01, sum23), _mm512_add_pd(sum45, sum67));
    let mut lanes = [0.0; 8];
    _mm512_storeu_pd(lanes.as_mut_ptr(), sum);
    black_box(lanes).into_iter().sum()
}

#[cfg(test)]
mod tests {
    use super::{format_cpu_list, measure, median};

    #[test]
    fn median_selects_middle_sample() {
        let values = [4.0, 1.0, 5.0, 2.0, 3.0];
        assert_eq!(median(&values), 3.0);
    }

    #[test]
    fn formats_cpu_affinity_ranges() {
        assert_eq!(format_cpu_list(&[0, 1, 2, 4, 6, 7]), "0-2,4,6-7");
    }

    #[test]
    #[ignore = "allocates the full calibration working set and measures the host"]
    fn host_calibration_smoke() {
        let calibration = measure().unwrap();
        eprintln!("{calibration:#?}");
        assert!(calibration.threads > 0);
        assert!(calibration.fp64_gflops.is_finite() && calibration.fp64_gflops > 0.0);
        assert!(
            calibration.memory_gbytes_per_second.is_finite()
                && calibration.memory_gbytes_per_second > 0.0
        );
        assert!(
            calibration.ridge_point_flops_per_byte.is_finite()
                && calibration.ridge_point_flops_per_byte > 0.0
        );
    }
}
