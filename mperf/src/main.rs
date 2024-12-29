mod record;
mod stat;

use clap::{Parser, Subcommand, ValueEnum};
use record::do_record;
use stat::do_stat;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    List,
    Stat {
        #[arg(last = true)]
        command: Vec<String>,
    },
    Record {
        #[arg(short, long)]
        scenario: Scenario,
        #[arg(short, long)]
        output_directory: String,
        #[arg(short, long)]
        pid: Option<usize>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Show,
}

#[derive(Clone, Debug, Copy, ValueEnum, PartialEq, Eq)]
enum Scenario {
    Snapshot,
    Roofline,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();

    match args.command {
        Commands::Stat { command } => {
            return do_stat(command);
        }
        Commands::List => {
            let events = pmu::list_counters();
            for event in events {
                println!("{} - {}", event.name(), event.description());
            }
        }
        Commands::Record {
            scenario,
            output_directory,
            pid,
            command,
        } => {
            if std::fs::exists(&output_directory)? {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("'{output_directory}' already exists"),
                )));
            }
            std::fs::create_dir_all(&output_directory)?;
            return do_record(scenario, output_directory, pid, command);
        }
        Commands::Show => {
            println!("Show data")
        }
    }

    Ok(())
}
