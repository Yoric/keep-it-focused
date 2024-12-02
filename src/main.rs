use std::{path::PathBuf, thread};

use anyhow::Context;
use clap::Parser;
use log::{info, set_max_level, warn, LevelFilter};
use systemd_journal_logger::{connected_to_journal, JournalLog};

const CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";

#[derive(Parser, Debug)]
struct Args {
    /// How often to check for offending processes.
    #[arg(short, long, default_value = "60")]
    sleep_s: u64,

    #[arg(short, long, default_value = CONFIG_PATH)]
    config: PathBuf,

    /// If true, remove any iptables configuration.
    #[arg(short, long, default_value = "false")]
    remove: bool,
}

fn main() -> Result<(), anyhow::Error> {
    if connected_to_journal() {
        eprintln!("using journal log");
        JournalLog::new().unwrap().install().unwrap();
        set_max_level(LevelFilter::Debug);
    } else {
        eprintln!("using simple logger");
        simple_logger::SimpleLogger::new().env().init().unwrap();
    }

    let args = Args::parse();
    eprintln!("args: {:?}", args);
    if args.remove {
        keep_it_focused::remove_ip_tables(keep_it_focused::IP_TABLES_PREFIX)?;
        return Ok(())
    }
    info!("loop: {}", "starting");


    let mut last_modified = std::fs::metadata(&args.config)
        .context("could not find configuration")?
        .modified()
        .context("no latest modification time")?;
    let mut config = keep_it_focused::init(&args.config).context("could not apply configuration")?;
    keep_it_focused::setup_ip_tables(&config).context("error while setting up iptables")?;

    loop {
        info!("loop: {}", "sleeping");
        thread::sleep(std::time::Duration::from_secs(args.sleep_s));
        let modified = std::fs::metadata(&args.config)
            .context("could not find configuration")?
            .modified()
            .context("no latest modification time")?;
        if modified > last_modified {
            info!("loop: {}", "reloading config");
            last_modified = modified;
            match keep_it_focused::init(&args.config) {
                Ok(new_config) => {
                    info!("loop: {}", "applying new configuration");
                    config = new_config;
                    keep_it_focused::setup_ip_tables(&config).context("error while setting up iptables")?;
                }
                Err(err) => warn!(
                    "error applying configuration, keeping old configuration {}",
                    err
                ),
            }
        }
        keep_it_focused::find_offending_processes(&config).context("error while examining offending processes")?;
    }
}
