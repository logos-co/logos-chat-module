#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mailbox_relay::run_from_env().await
}
