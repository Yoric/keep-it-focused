use std::{collections::HashMap, path::{Path, PathBuf}, rc::Rc, thread};

use anyhow::Context;
use clap::Parser;
use config::{Binary, DayOfWeek, TimeOfDay};
use log::{debug, info, set_max_level, warn, LevelFilter};
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

    #[arg(short, long, default_value = CONFIG_PATH)]
    config: PathBuf,
}

pub fn main() -> Result<(), anyhow::Error> {
    if connected_to_journal() {
        eprintln!("using journal log");
        JournalLog::new().unwrap().install().unwrap();
        set_max_level(LevelFilter::Debug);
    } else {
        eprintln!("using simple logger");
        simple_logger::SimpleLogger::new().env().init().unwrap();
    }
    info!("starting");

    let args = Args::parse();

    let mut last_modified = std::fs::metadata(&args.config)
        .context("could not find configuration")?
        .modified()
        .context("no latest modification time")?;
    let mut config = init(&args.config).context("could not read configuration")?;

    loop {
        info!("sleeping");
        thread::sleep(std::time::Duration::from_secs(args.sleep_s));
        let modified = std::fs::metadata(&args.config)
            .context("could not find configuration")?
            .modified()
            .context("no latest modification time")?;
        if modified > last_modified {
            info!("reloading config");
            last_modified = modified;
            config = init(&args.config).context("could not read configuration")?;
        }
        find_offending_processes(&config).context("error while examining offending processes")?;
    }
}

pub struct UserInstructions {
    user_name: Rc<String>,
    instructions: Vec<(Binary, Box<[config::Interval]>)>,
}
impl UserInstructions {
    pub fn new(user_name: Rc<String>) -> Self {
        UserInstructions {
            user_name,
            instructions: Vec::new(),
        }
    }
}
type PerUid = HashMap<Uid, UserInstructions>;
type PerDay = HashMap<DayOfWeek, PerUid>;

pub fn init<P: AsRef<Path>>(path: &P) -> Result<PerDay, anyhow::Error> {
    info!("reading config... start");
    let reader = std::fs::File::open(path)
        .with_context(|| format!("could not open file {}", path.as_ref().to_string_lossy()))?;
    let config: config::Config = serde_yaml::from_reader(reader)
        .with_context(|| format!("could not parse file {}", path.as_ref().to_string_lossy()))?;

    let mut resolver = uid_resolver::Resolver::new();
    let mut per_day: PerDay = HashMap::new();
    for (user, week) in config.users {
        let user = Rc::new(user);
        let uid = resolver.resolve(&user)?;
        for (day, day_config) in week.0 {
            let this_day = per_day.entry(day).or_default();
            let this_user = this_day
                .entry(uid)
                .or_insert_with(|| UserInstructions::new(user.clone()));
            for proc in day_config.processes {
                this_user
                    .instructions
                    .push((proc.binary, proc.permitted.into_boxed_slice()))
            }
        }
    }
    info!("reading config... complete");
    Ok(per_day)
}

pub fn find_offending_processes(config: &PerDay) -> Result<(), anyhow::Error> {
    let today = DayOfWeek::now();
    let Some(today_config) = config.get(&today) else {
        // Nothing to do for today.
        return Ok(());
    };
    if today_config.is_empty() {
        // Nothing to do for today.
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
                warn!(target: "procfs", "could not access proc, skipping: {}", err);
                continue;
            }
        };
        let uid = match proc.uid() {
            Ok(uid) => Uid(uid),
            Err(err) => {
                warn!(target: "procfs", "could not access proc uid for process {pid}, skipping: {}", err, pid=proc.pid);
                continue;
            }
        };
        let Some(user_config) = today_config.get(&uid) else {
            // Nothing to watch for this user.
            continue;
        };
        let exe = match proc.exe() {
            Ok(exe) => exe,
            Err(err @ ProcError::PermissionDenied(_)) => {
                debug!(target: "procfs", "could not access proc exe for process {pid}, skipping: {}", err, pid=proc.pid);
                continue;
            }
            Err(err) => {
                warn!(target: "procfs", "could not access proc exe for process {pid}, skipping: {}", err, pid=proc.pid);
                continue;
            }
        };

        debug!("examining process {:?}", exe);
        for (binary, intervals) in &user_config.instructions {
            if !binary.matcher.is_match(&exe) {
                debug!("we're not interested in this process");
                continue;
            }
            info!(
                "found binary {} for user {}",
                exe.to_string_lossy(),
                user_config.user_name
            );
            if let Some(duration) = intervals
                .iter()
                .filter_map(|interval| interval.remaining(now))
                .next()
            {
                // We're still in permitted territory.
                info!("binary is still allowed at this time");
                if duration < std::time::Duration::from_secs(300) {
                    // ...however, we're less than 5 minutes away from shutdown, so let's warn user!
                    let minutes = duration.as_secs() / 60;
                    if let Err(err) = notify(
                        &user_config.user_name,
                        &format!("{} will quit in {} minutes", exe.to_string_lossy(), minutes),
                        notify::Urgency::Significant,
                    ) {
                        warn!(target: "notify", "failed to notify user {}: {:?}", user_config.user_name, err)
                    }
                }
            } else {
                info!("let's kill this binary");
                // Time to kill the binary.
                if let Err(err) = notify(
                    &user_config.user_name,
                    &format!(
                        "{} is not permitted at this time, stopping it",
                        exe.to_string_lossy()
                    ),
                    notify::Urgency::Significant,
                ) {
                    warn!(target: "notify", "failed to notify user {}: {:?}", user_config.user_name, err)
                }
                if let Err(err) = kill_tree::blocking::kill_tree_with_config(
                    proc.pid as u32,
                    &kill_tree::Config {
                        signal: "SIGKILL".to_string(),
                        ..Default::default()
                    },
                ) {
                    warn!(target: "notify", "failed to kill process {}: {:?}", exe.to_string_lossy(), err)
                }
                info!("binary killed");
            }
        }
    }
    Ok(())
}
