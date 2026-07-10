use pmu::{Counter, EventTimer};

fn hot_block(values: &mut [u64]) {
    for (index, value) in values.iter_mut().enumerate() {
        *value = value.wrapping_add(index as u64).rotate_left(7);
    }
}

fn main() -> Result<(), pmu::Error> {
    let timer = EventTimer::new(&[Counter::Cycles, Counter::Instructions, Counter::LLCMisses])?;
    let mut values = vec![1_u64; 16 * 1024];

    let span = timer.start()?;
    hot_block(&mut values);
    let measurement = span.stop()?;

    println!(
        "{} cycles, {} instructions, IPC {:.2}, {} ns (snapshot cost: {} ns via {:?})",
        measurement[Counter::Cycles],
        measurement[Counter::Instructions],
        measurement.ipc(),
        measurement.wall_ns(),
        timer.read_cost().nanoseconds(),
        timer.read_cost().method(),
    );

    let stats = timer.measure_n("hot_block", 100, || hot_block(&mut values))?;
    let cycles = &stats[Counter::Cycles];
    println!(
        "{} (n={}): cycles min={} mean={:.0} p50={} p99={}",
        stats.label(),
        stats.iterations(),
        cycles.min(),
        cycles.mean(),
        cycles.p50(),
        cycles.p99(),
    );
    std::hint::black_box(values);
    Ok(())
}
