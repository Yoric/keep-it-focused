mod config;
mod notify;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    notify::notify("test message", notify::Urgency::Critical, std::time::Duration::from_millis(10_000)).await?;
    Ok(())
}
