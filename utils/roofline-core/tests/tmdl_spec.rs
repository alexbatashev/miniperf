#[path = "../tmdl_spec.rs"]
mod tmdl_spec;

#[test]
fn vendored_spec_is_valid_and_generation_is_deterministic() {
    let temporary = std::env::temp_dir();
    let first = temporary.join(format!("miniperf-tmdl-{}-first.rs", std::process::id()));
    let second = temporary.join(format!("miniperf-tmdl-{}-second.rs", std::process::id()));
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/spec/riscv-ast-v1.json");

    tmdl_spec::generate(input, &first).unwrap();
    tmdl_spec::generate(input, &second).unwrap();
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap()
    );

    std::fs::remove_file(first).unwrap();
    std::fs::remove_file(second).unwrap();
}
