#[tokio::main]
async fn main() -> anyhow::Result<()> {
    flakyfy::run().await
}
