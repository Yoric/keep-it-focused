use std::{collections::HashMap, path::Path, thread};

use anyhow::Context;
use clap::Parser;
use config::{Binary, TimeOfDay};
use log::info;
use notify::notify;
use procfs::ProcError;
use systemd_journal_logger::{connected_to_journal, JournalLog};
use uid_resolver::Uid;

mod config;
mod notify;
mod uid_resolver;

const CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";

#[derive(Parser, Debug)]
pub struct Args {
    /// How often to check for offending processes.
    #[arg(short, long, default_value = "60")]
    sleep_s: u64,
}

pub fn main() -> Result<(), anyhow::Error> {
    if connected_to_journal() {
        JournalLog::new().unwrap().install().unwrap();
    } else {
        simple_logger::SimpleLogger::new().env().init().unwrap();
    }
    let args = Args::parse();

    let mut last_modified = std::fs::metadata(CONFIG_PATH)
        .context("could not find configuration")?
        .modified()
        .context("no latest modification time")?;
    let mut config = init(&CONFIG_PATH).context("could not read configuration")?;

    loop {
        info!("sleeping");
        thread::sleep(std::time::Duration::from_secs(args.sleep_s));
        let modified = std::fs::metadata(CONFIG_PATH)
            .context("could not find configuration")?
            .modified()
            .context("no latest modification time")?;
        if modified > last_modified {
            info!("reloading config");
            last_modified = modified;
            config = init(&CONFIG_PATH).context("could not read configuration")?;
        }
        find_offending_processes(&config).context("error while examining offending processes")?;
    }
}

pub struct UserInstructions {
    user_name: String,
    instructions: Vec<(Binary, Box<[config::Interval]>)>,
}
impl UserInstructions {
    pub fn new(user_name: String) -> Self {
        UserInstructions {
            user_name,
            instructions: Vec::new(),
        }
    }
}
type PerUid = HashMap<Uid, UserInstructions>;

pub fn init<P: AsRef<Path>>(path: &P) -> Result<PerUid, anyhow::Error> {
    let reader = std::fs::File::open(path)
        .with_context(|| format!("could not open file {}", path.as_ref().to_string_lossy()))?;
    let config: config::Config = serde_yaml::from_reader(reader)
        .with_context(|| format!("could not parse file {}", path.as_ref().to_string_lossy()))?;

    let mut resolver = uid_resolver::Resolver::new();
    let mut per_uid = HashMap::new();
    for watch in config.watch {
        let uid = resolver.resolve(&watch.user)?;
        let per_binary = per_uid
            .entry(uid)
            .or_insert_with(|| UserInstructions::new(watch.user));
        per_binary
            .instructions
            .push((watch.binary, watch.permitted.into_boxed_slice()));
    }
    Ok(per_uid)
}

pub fn find_offending_processes(config: &PerUid) -> Result<(), anyhow::Error> {
    if config.is_empty() {
        return Ok(());
    }

    let now = TimeOfDay::from(chrono::Local::now());
    let processes = procfs::process::all_processes()
        .context("could not access /proc, is this a Linux machine?")?;

    for proc in processes {
        // Examine process.
        let proc = match proc {
            Ok(p) => p,
            Err(err) => {
                log::warn!(target: "procfs", "could not access proc: {}", err);
                continue;
            }
        };
        let uid = match proc.uid() {
            Ok(uid) => Uid(uid),
            Err(err) => {
                log::warn!(target: "procfs", "could not access proc uid for process {pid}: {}", err, pid=proc.pid);
                continue;
            }
        };
        let Some(per_binary) = config.get(&uid) else {
            // Nothing to watch for this user.
            continue;
        };
        let exe = match proc.exe() {
            Ok(exe) => exe,
            Err(err @ ProcError::PermissionDenied(_)) => {
                log::debug!(target: "procfs", "could not access proc exe for process {pid}: {}", err, pid=proc.pid);
                continue;
            }
            Err(err) => {
                log::warn!(target: "procfs", "could not access proc exe for process {pid}: {}", err, pid=proc.pid);
                continue;
            }
        };

        for (binary, intervals) in &per_binary.instructions {
            if !binary.matcher.is_match(&exe) {
                continue;
            }
            log::info!("found binary {} for user {}", exe.to_string_lossy(), per_binary.user_name);
            if let Some(duration) = intervals
                .iter()
                .filter_map(|interval| interval.remaining(now))
                .next()
            {
                // We're still in permitted territory.
                log::info!("binary is still allowed at this time");
                if duration < std::time::Duration::from_secs(300) {
                    // ...however, we're less than 5 minutes away from shutdown, so let's warn user!
                    let minutes = duration.as_secs() / 60;
                    if let Err(err) = notify(
                        &per_binary.user_name,
                        &format!("{} will quit in {} minutes", exe.to_string_lossy(), minutes),
                        notify::Urgency::Significant,
                    ) {
                        log::warn!(target: "notify", "failed to notify user {}: {:?}", per_binary.user_name, err)
                    }
                }
            } else {
                log::info!("let's kill this binary");
                // Time to kill the binary.
                if let Err(err) = notify(
                    &per_binary.user_name,
                    &format!("{} is not permitted at this time, stopping it", exe.to_string_lossy()),
                    notify::Urgency::Significant,
                ) {
                    log::warn!(target: "notify", "failed to notify user {}: {:?}", per_binary.user_name, err)
                }
                if let Err(err) = kill_tree::blocking::kill_tree_with_config(
                    proc.pid as u32,
                    &kill_tree::Config {
                        signal: "SIGKILL".to_string(),
                        ..Default::default()
                    },
                ) {
                    log::warn!(target: "notify", "failed to kill process {}: {:?}", exe.to_string_lossy(), err)
                }
                log::info!("binary killed");
            }
        }
    }
    Ok(())
}
