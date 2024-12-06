use std::{
    io::{BufRead, BufReader, Cursor},
    ops::Not,
    process::Command,
    rc::Rc,
};

use anyhow::Context;
use itertools::Itertools;
use lazy_regex::lazy_regex;
use log::{debug, warn};

use crate::{
    types::{TimeOfDay, DAY_ENDS},
    uid_resolver::Uid,
};

#[derive(typed_builder::TypedBuilder)]
pub struct IPTable {
    #[builder(default=Rc::new("filter".to_string()))]
    table: Rc<String>,
}

#[derive(Debug)]
pub enum Filter<'a> {
    Time {
        start: Option<TimeOfDay>,
        end: Option<TimeOfDay>,
    },
    Owner {
        uid: Uid,
    },
    Source {
        domain: &'a str,
    },
    Destination {
        domain: &'a str,
    },
}

fn iptables() -> Command {
    Command::new("iptables")
}
fn run(mut command: Command) -> Result<Vec<u8>, anyhow::Error> {
    let args = command
        .get_args()
        .map(|s| s.to_string_lossy().to_string())
        .collect_vec();
    let output = command
        .output()
        .with_context(|| format!("failed to launch iptables command {:?}", args))?;
    if output.status.success().not() {
        let err = String::from_utf8_lossy(&output.stderr);
        warn!("iptables failed {}", err);
        let err = match output.status.code() {
            None => anyhow::anyhow!(
                "iptables command interrupted by signal {:?}: {}",
                args,
                output.status.to_string()
            ),
            Some(code) => anyhow::anyhow!(
                "error ({code}: {}) executing iptables command {:?}: {}",
                errno::Errno(code),
                args,
                output.status.to_string()
            ),
        };
        return Err(err);
    }
    Ok(output.stdout)
}

impl IPTable {
    pub fn list(self, zero: bool, prefix: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let mut command = iptables();
        command.args(["--table", &self.table, "--list"]);
        if zero {
            command.arg("--zero");
        }
        let out = String::from_utf8_lossy(&run(command)?).to_string();
        let mut instances = vec![];
        let mut by_line = BufReader::new(Cursor::new(out));
        loop {
            let mut line: String = String::new();
            if let Ok(0) = by_line.read_line(&mut line) {
                return Ok(instances);
            }
            debug!("reading {:?}", line);
            let re = lazy_regex!("Chain ([A-Za-z0-9-]+) ");
            let Some(captures) = re.captures(&line) else {
                continue;
            };
            debug!("captures {:?}", captures);
            let (_, [chain_name]) = captures.extract();
            if let Some(prefix) = prefix {
                if chain_name.starts_with(prefix).not() {
                    continue;
                };
            }
            debug!("we're interested in chain {:?}", chain_name);
            instances.push(chain_name.to_string());
        }
    }
    pub fn flush(self, chain: &str) -> Result<(), anyhow::Error> {
        let mut command = iptables();
        command.args(["--table", &self.table, "--flush", chain]);
        run(command)?;
        Ok(())
    }
    pub fn delete(self, chain: &str) -> Result<(), anyhow::Error> {
        let mut command = iptables();
        command.args(["--table", &self.table, "--delete-chain", chain]);
        run(command)?;
        Ok(())
    }
    pub fn create(self, chain: &str) -> Result<Chain, anyhow::Error> {
        let mut command = iptables();
        command.args(["--table", &self.table, "--new-chain", chain]);
        run(command)?;
        Ok(Chain {
            table: self.table.clone(),
            name: chain,
        })
    }
}

pub enum Finish {
    Drop,
}

pub struct Chain<'a> {
    table: Rc<String>,
    name: &'a str,
}
impl Chain<'_> {
    pub fn append(&mut self, filter: Filter) -> Result<(), anyhow::Error> {
        let mut command = iptables();
        command.args(["--table", &self.table, "--append", self.name]);
        match filter {
            Filter::Time {
                start: None,
                end: None,
            } => {
                // Nothing to do
                return Ok(());
            }
            Filter::Time { start, end } => {
                command.args(["--match", "time"]);
                if let Some(start) = start {
                    command.args(["--timestart", &start.as_iptables_arg()]);
                }
                if let Some(end) = end {
                    if end != DAY_ENDS {
                        command.args(["--timestop", &end.as_iptables_arg()]);
                    }
                }
            }
            Filter::Owner { uid } => {
                command.args(["--match", "owner", "--uid-owner", &format!("{}", uid.0)]);
            }
            Filter::Source { domain } => {
                command.args(["--source", domain]);
            }
            Filter::Destination { domain } => {
                command.args(["--destination", domain]);
            }
        }
        run(command)?;
        Ok(())
    }
    pub fn finish(self, finish: Finish) -> Result<(), anyhow::Error> {
        let jump = match finish {
            Finish::Drop => "DROP",
        };
        let mut command = iptables();
        command.args([
            "--table",
            &self.table,
            "--append",
            self.name,
            "--jump",
            jump,
        ]);
        run(command)?;
        Ok(())
    }
}
