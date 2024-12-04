use std::{path::PathBuf, thread};

use anyhow::Context;
use clap::Parser;
use log::{info, set_max_level, warn, LevelFilter};
use systemd_journal_logger::{connected_to_journal, JournalLog};

const DEFAULT_CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";
const DEFAULT_PORT: &str = "7878";

#[derive(Parser, Debug)]
struct Args {
    /// How often to check for offending processes.
    #[arg(short, long, default_value = "60")]
    sleep_s: u64,

    #[arg(short, long, default_value = DEFAULT_CONFIG_PATH)]
    config: PathBuf,

    #[arg(short, long, default_value = DEFAULT_PORT)]
    port: u16,

    /// If true, remove any iptables configuration.
    #[arg(short, long, default_value = "false")]
    remove: bool,

    #[arg(short, long, default_value = "false")]
    ip_tables: bool,
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
    let mut focuser = keep_it_focused::KeepItFocused::try_new(keep_it_focused::Options { ip_tables: args.ip_tables, port: args.port, path: args.config })
        .context("failed to apply configuration")?;
    focuser.background_serve();

    loop {
        info!("loop: {}", "sleeping");
        thread::sleep(std::time::Duration::from_secs(args.sleep_s));
        if let Err(err) = focuser.tick() {
            warn!("problem during tick {}", err);
        }
    }
}
