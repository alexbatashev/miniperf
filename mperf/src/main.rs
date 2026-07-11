mod counter_selection;
mod disassembly;
mod event_dispatcher;
mod events_export;
mod postprocess;
mod processing;
mod record;
mod stat;
mod tui;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod unwind;
mod utils;

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::Result;
use clap::{Parser, Subcommand};

use events_export::do_events_export;
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
        #[arg(short, long)]
        pid: Option<u32>,
        /// Comma-separated event names (use `mperf list` to discover them).
        #[arg(short = 'e', long = "event", value_delimiter = ',')]
        events: Vec<String>,
        /// Render the host Top-down methodology instead of the flat stat table.
        #[arg(long)]
        topdown: bool,
        /// Maximum Top-down tree level to display (default: 1).
        #[arg(short = 'l', long, default_value_t = 1)]
        level: u8,
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
    Show {
        result_directory: String,
    },
    EventsExport {
        result_directory: String,
    },
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    let args = Cli::parse();

    match args.command {
        Commands::Stat {
            pid,
            events,
            topdown,
            level,
            command,
        } => return do_stat(pid, command, events, topdown.then_some(level)),
        Commands::List => {
            let events = pmu::list_supported_counters(pmu::DriverKind::Default);
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
        Commands::Show { result_directory } => {
            let path = Path::new(&result_directory);
            return tui::tui_main(path).await;
        }
        Commands::EventsExport { result_directory } => {
            let path = Path::new(&result_directory);
            do_events_export(path);
        }
    }

    Ok(())
}
