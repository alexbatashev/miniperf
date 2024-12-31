mod event_dispatcher;
mod record;
mod stat;

use std::{path::PathBuf, str::FromStr};

use anyhow::Result;
use clap::{Parser, Subcommand};

use mperf_data::Scenario;
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
        pid: Option<u32>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Show,
}

#[tokio::main]
async fn main() -> Result<()> {
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
                return Err(Into::<anyhow::Error>::into(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("'{output_directory}' already exists"),
                ))
                .context("profiling results must be put in different directories"));
            }
            std::fs::create_dir_all(&output_directory)?;

            let output_directory = PathBuf::from_str(&output_directory)?;

            return do_record(scenario, &output_directory, pid, command).await;
        }
        Commands::Show => {
            println!("Show data")
        }
    }

    Ok(())
}
