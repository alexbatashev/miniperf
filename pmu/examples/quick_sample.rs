use pmu::{top_symbols, Counter, QuickSampler, UnwindMode};

fn main() -> Result<(), pmu::Error> {
    let sampler =
        QuickSampler::bounded(&[Counter::Cycles], 10_000)?.unwind_mode(UnwindMode::FramePointer);
    let batch = sampler.record_batch(4_000, || {
        let mut value = 1_u64;
        for index in 0..50_000_000_u64 {
            value = std::hint::black_box(value.wrapping_add(index).rotate_left(5));
        }
        std::hint::black_box(value);
    })?;

    println!(
        "{} samples ({} dropped)",
        batch.len(),
        batch.dropped_samples()
    );
    for entry in top_symbols(batch.samples(), 10) {
        println!("{:>8}  {}", entry.samples(), entry.symbol());
    }
    Ok(())
}
