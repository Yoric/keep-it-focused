use anyhow::Context;

pub enum Urgency {
    Low,
    Significant,
    Critical
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

pub async fn notify(message: &str, urgency: Urgency, duration: std::time::Duration) -> Result<(), anyhow::Error> {
    // Find dbus address.
    let uid = unsafe { libc::geteuid() };
    let dbus_address = format!("unix:path=/run/user/{uid}/bus");

    // Find displays.
    let who_regex = lazy_regex::regex!{"^([^ ]+).*\\(([^)]+)\\)$"m};
    let who_output = tokio::process::Command::new("who")
        .output()
        .await
        .context("Failed to launch who")?;
    let who_stdout = String::from_utf8(who_output.stdout)
        .context("Invalid who output")?;
    eprintln!("stdout {:?}", who_stdout);
    for capture in who_regex.captures_iter(&who_stdout) {
        let [user, display] = capture.extract::<2>().1;
        eprintln!("user={}", user);
        eprintln!("display={}", display);

        let out = tokio::process::Command::new("sudo")
            .args(["-u", user])
            .arg(format!("DISPLAY={display}"))
            .arg(format!("DBUS_SESSION_BUS_ADDRESS={dbus_address}"))
            .arg("notify-send")
            .arg("--app-name='Let\'s take a break'")
            .arg(format!("--urgency={urgency}"))
            .arg(format!("--expire-time={}", duration.as_millis()))
            .arg(message)
            .output()
            .await
            .context("Failed to launch sudo or notify")?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        eprintln!("out: {stdout}\nerr: {stderr}");
    }

    Ok(())
}