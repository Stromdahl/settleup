//! The HTTP route table, kept separate from `main` so the wiring of paths to
//! handlers lives in one place.

use axum::Router;
use axum::routing::{get, post};

use crate::handlers::{self, AppState};

/// Build the application router with all routes wired to their handlers and the
/// shared [`AppState`] attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::landing).post(handlers::create_group))
        .route("/assets/htmx-2.0.4.min.js", get(handlers::htmx_js))
        .route("/g/{id}", get(handlers::group_page))
        .route("/g/{id}/add", get(handlers::add_expense_page))
        .route("/g/{id}/live", get(handlers::live))
        .route("/g/{id}/join", post(handlers::join_group))
        .route("/g/{id}/expenses", post(handlers::add_expense))
        .route(
            "/g/{id}/expenses/{eid}/edit",
            get(handlers::edit_expense_page).post(handlers::edit_expense),
        )
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
        .with_state(state)
}
