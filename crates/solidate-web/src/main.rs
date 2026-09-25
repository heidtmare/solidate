use solidate_app::db::Db;
use solidate_app::{App, Config, Oidc, OidcConfig, telemetry};
use solidate_web::WebConfig;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    telemetry::init("info");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let config = Config::from_env().unwrap_or_else(|e| panic!("invalid configuration: {e}"));
    let db = Db::connect(&url).await.expect("database connection");
    let web = WebConfig {
        insecure_cookies: std::env::var("SOLIDATE_INSECURE_COOKIES").is_ok_and(|v| v == "1"),
        public_url: std::env::var("SOLIDATE_PUBLIC_URL")
            .ok()
            .map(|u| u.trim_end_matches('/').to_owned()),
    };
    let mut app = App::new(db, config);
    let oidc = OidcConfig::from_env(web.public_url.as_deref()).unwrap_or_else(|e| panic!("invalid configuration: {e}"));
    if let Some(oidc) = oidc {
        let issuer = oidc.issuer.clone();
        let oidc = Oidc::discover(oidc)
            .await
            .unwrap_or_else(|e| panic!("OpenID Connect provider {issuer}: {e}"));
        tracing::info!(%issuer, "OpenID Connect sign-in enabled");
        app = app.with_oidc(oidc);
    }
    tracing::info!(
        host = std::env::var("HOST").as_deref().unwrap_or("127.0.0.1"),
        port = std::env::var("PORT").as_deref().unwrap_or("3000"),
        "starting solidate-server"
    );
    topcoat::start(solidate_web::router(app, web)).await.expect("server");
}
