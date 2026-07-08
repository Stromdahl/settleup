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
        .nest("/g", group_routes())
        .with_state(state)
}

/// Routes scoped to a single group, mounted under `/g` — so each path here is
/// relative to that prefix (`/{id}` serves `/g/{id}`, and so on).
fn group_routes() -> Router<AppState> {
    Router::new()
        .route("/{id}", get(handlers::group_page))
        .route("/{id}/add", get(handlers::add_expense_page))
        .route("/{id}/live", get(handlers::live))
        .route("/{id}/join", post(handlers::join_group))
        .route("/{id}/expenses", post(handlers::add_expense))
        .route(
            "/{id}/expenses/{eid}/edit",
            get(handlers::edit_expense_page).post(handlers::edit_expense),
        )
        .route(
            "/{id}/expenses/{eid}/delete",
            post(handlers::delete_expense),
        )
        .route("/{id}/settlements", post(handlers::mark_settlement))
        .route("/{id}/close", post(handlers::close_group))
        .route("/{id}/reopen", post(handlers::reopen_group))
        .route("/{id}/recovery", post(handlers::set_recovery))
        .route(
            "/{id}/recover",
            get(handlers::recover_page).post(handlers::recover_submit),
        )
}
