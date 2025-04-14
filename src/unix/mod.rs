use anyhow::{Context, Error};

#[cfg(target_os = "linux")]
pub mod linux;
pub mod uid_resolver;

pub fn kill_process(pid: uid_resolver::Pid) -> Result<(), Error> {
    let mut cmd = std::process::Command::new("kill");
    cmd.arg("-9")
        .arg(format!("{}", pid.0));
    cmd.output()
        .with_context(|| format!("failed to kill process {}", pid.0))?;
    Ok(())
}