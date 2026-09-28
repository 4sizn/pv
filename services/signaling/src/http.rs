//! HTTP and upgrade boundary. Owns request validation; store owns authentication state.
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::{
    ice::ice_servers,
    models::{RoomCredentials, ServiceError},
    state::AppState,
    ws,
};

pub fn router(state: AppState) -> Router {
    let origins: Vec<HeaderValue> = state
        .config
        .allowed_origins
        .iter()
        .map(|value| value.parse().expect("validated origin header"))
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    Router::new()
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/devices", post(create_device))
        .route("/rooms", post(create_room))
        .route("/ws", get(upgrade))
        .layer(cors)
        .with_state(state)
}

async fn create_device(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_allowed(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match state.create_device(Instant::now()) {
        Ok(device) => (StatusCode::CREATED, Json(device)).into_response(),
        Err(error) => service_error(error),
    }
}

async fn create_room(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !origin_allowed(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty());
    let Some(token) = token else {
        return service_error(ServiceError::Unauthorized);
    };
    match state.create_room(token, Instant::now()) {
        Ok((room_id, room_token, peer_id)) => {
            let unix_seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            (
                StatusCode::CREATED,
                Json(RoomCredentials {
                    room_id,
                    room_token,
                    ice_servers: ice_servers(&state.config, &peer_id, unix_seconds),
                }),
            )
                .into_response()
        }
        Err(error) => service_error(error),
    }
}

async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = state.connection_slots.clone().try_acquire_owned() else {
        return service_error(ServiceError::Capacity);
    };
    ws.max_message_size(state.config.max_message_bytes)
        .max_frame_size(state.config.max_message_bytes)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            ws::serve(socket, state).await;
        })
        .into_response()
}

fn origin_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    // Native clients may omit Origin; credentials are still required for room use.
    headers.get(header::ORIGIN).is_none_or(|origin| {
        origin.to_str().ok().is_some_and(|origin| {
            state
                .config
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        })
    })
}

fn service_error(error: ServiceError) -> Response {
    let status = match error {
        ServiceError::Unauthorized => StatusCode::UNAUTHORIZED,
        ServiceError::Capacity => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(error.wire())).into_response()
}
