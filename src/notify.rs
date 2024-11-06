use anyhow::Context;
use log::info;

#[allow(dead_code)]
pub enum Urgency {
    Low,
    Significant,
    Critical,
}
impl std::fmt::Display for Urgency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match *self {
            Urgency::Low => "low",
            Urgency::Significant => "normal",
            Urgency::Critical => "critical",
        };
        write!(f, "{str}")
    }
}

pub fn notify(user: &str, message: &str, urgency: Urgency) -> Result<(), anyhow::Error> {
    info!("attempting to notify {user} of message {message}");
    let _ = std::process::Command::new("systemd-run")
        .arg("--user")
        .arg(format!("--machine={user}@.host"))
        .arg("notify-send")
        .arg(format!("--urgency={urgency}"))
        .arg("--app-name='Let\'s take a break'")
        .arg(message)
        .output()
        .context("Failed to launch systemd-run or notify-send")?;

    Ok(())
}
