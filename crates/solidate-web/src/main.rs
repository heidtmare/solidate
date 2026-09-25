use solidate_app::db::Db;
use solidate_app::{App, Config};
use solidate_web::WebConfig;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = Db::connect(&url).await.expect("database connection");
    let app = App::new(db, Config::default());
    let web = WebConfig {
        insecure_cookies: std::env::var("SOLIDATE_INSECURE_COOKIES").is_ok_and(|v| v == "1"),
    };
    topcoat::start(solidate_web::router(app, web)).await.expect("server");
}
