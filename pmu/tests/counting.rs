use pmu::Counter;

#[test]
fn simple_counters() {
    let mut vec = (0..=100000).collect::<Vec<_>>();

    let mut driver = pmu::CountingDriver::new(&[Counter::Cycles, Counter::Instructions], None)
        .expect("driver creation");

    driver.start().expect("start");
    vec.sort();
    driver.stop().expect("stop");

    let counters = driver.counters().expect("counters");
    assert_ne!(counters.get(Counter::Cycles).unwrap().value, 0);
    assert_ne!(counters.get(Counter::Instructions).unwrap().value, 0);
}
