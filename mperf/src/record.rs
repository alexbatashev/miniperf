use crate::Scenario;

pub fn do_record(scenario: Scenario, _output_directory: String, _pid: Option<usize>, _command: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Record profile with {scenario:?}");
    Ok(())
}
