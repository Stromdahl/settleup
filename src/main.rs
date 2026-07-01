mod db;
mod handlers;
mod ids;
mod models;
mod money;
mod settle;
mod views;

use axum::Router;
use axum::routing::{get, post};
use handlers::AppState;

/// Groups with no recovery passphrase are auto-deleted after this many days of
/// inactivity, so throwaway bar tabs clean themselves up.
const INACTIVE_DAYS: i64 = 3;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = std::env::var("SETTLEUP_DB").unwrap_or_else(|_| "settleup.db".into());
    let base_url = std::env::var("SETTLEUP_BASE_URL").ok();
    let addr = std::env::var("SETTLEUP_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());

    let pool = db::connect(&db_path).await?;

    // Background auto-expiry of stale, unclaimed groups (first sweep at startup).
    {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                match db::expire_stale_groups(&pool, INACTIVE_DAYS).await {
                    Ok(n) if n > 0 => println!("expired {n} stale group(s)"),
                    Ok(_) => {}
                    Err(e) => eprintln!("expiry error: {e}"),
                }
            }
        });
    }

    let state = AppState { pool, base_url };
    let app = Router::new()
        .route("/", get(handlers::landing).post(handlers::create_group))
        .route("/g/{id}", get(handlers::group_page))
        .route("/g/{id}/join", post(handlers::join_group))
        .route("/g/{id}/expenses", post(handlers::add_expense))
        .route(
            "/g/{id}/expenses/{eid}/delete",
            post(handlers::delete_expense),
        )
        .route("/g/{id}/settlements", post(handlers::mark_settlement))
        .route("/g/{id}/close", post(handlers::close_group))
        .route("/g/{id}/reopen", post(handlers::reopen_group))
        .route("/g/{id}/recovery", post(handlers::set_recovery))
        .route(
            "/g/{id}/recover",
            get(handlers::recover_page).post(handlers::recover_submit),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("SettleUp listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
