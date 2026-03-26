use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), fluxa_backend::error::AppError> {
    dotenvy::dotenv().ok();
    let cli = fluxa_backend::config::Cli::parse();
    fluxa_backend::run(cli).await
}
