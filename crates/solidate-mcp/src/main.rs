//! `solidate-mcp`: MCP server over stdio. Environment: `DATABASE_URL`,
//! `SOLIDATE_TOKEN` (an API token; its scopes and project restriction apply).

use rmcp::ServiceExt;
use solidate_app::db::Db;
use solidate_app::{App, Config, telemetry};
use solidate_mcp::SolidateMcp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    telemetry::init("warn");
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL must be set")?;
    let token = std::env::var("SOLIDATE_TOKEN").map_err(|_| "SOLIDATE_TOKEN must be set")?;
    let app = App::new(Db::connect(&url).await?, Config::from_env()?);
    // Fail fast on a bad token instead of on the first tool call.
    app.token_ctx(&token)
        .await
        .map_err(|e| format!("SOLIDATE_TOKEN: {e}"))?;
    let server = SolidateMcp::with_token(app, token)
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}
