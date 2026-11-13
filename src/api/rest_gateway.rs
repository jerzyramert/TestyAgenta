use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use uuid::Uuid;

use super::{
    to_api_result, ApiState, CalculateRouteInput, JobStatus, PolarValidationInput, RouteJob,
    ASYNC_ROUTE_THRESHOLD_M,
};
use crate::error::DomainError;

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/routes/calculate", post(calculate_route))
        .route("/v1/routes/:job_id", get(get_route_job))
        .route("/v1/polars/validate", post(validate_polar))
        .with_state(state)
}

async fn calculate_route(
    State(state): State<ApiState>,
    Json(input): Json<CalculateRouteInput>,
) -> Result<Response, ApiError> {
    let distance_m = ApiState::route_distance_m(&input)?;
    if distance_m < ASYNC_ROUTE_THRESHOLD_M {
        let route = tokio::task::spawn_blocking(move || {
            let result = state.calculate(&input)?;
            to_api_result(&result)
        })
        .await
        .map_err(|error| ApiError(DomainError::RouteJobStore(error.to_string())))??;
        return Ok((StatusCode::OK, Json(route)).into_response());
    }

    let job_id = Uuid::new_v4().to_string();
    let initial_job = RouteJob {
        status: JobStatus::Processing,
        result: None,
        error: None,
    };
    let store = state.store.clone();
    let initial_id = job_id.clone();
    tokio::task::spawn_blocking(move || store.save_job(&initial_id, &initial_job))
        .await
        .map_err(|error| ApiError(DomainError::RouteJobStore(error.to_string())))??;

    let task_job_id = job_id.clone();
    let _job = tokio::task::spawn_blocking(move || {
        let job = match state
            .calculate(&input)
            .and_then(|result| to_api_result(&result))
        {
            Ok(result) if result.status == "COMPLETED" => RouteJob {
                status: JobStatus::Completed,
                result: Some(result),
                error: None,
            },
            Ok(result) => RouteJob {
                status: JobStatus::Failed,
                result: Some(result),
                error: Some("ERR_NO_SAFE_ROUTE_FOUND".to_owned()),
            },
            Err(error) => RouteJob {
                status: JobStatus::Failed,
                result: None,
                error: Some(error.to_string()),
            },
        };
        if let Err(error) = state.store.save_job(&task_job_id, &job) {
            eprintln!("failed to persist route job {task_job_id}: {error}");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"job_id": job_id, "status": "PROCESSING"})),
    )
        .into_response())
}

async fn get_route_job(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
) -> Result<Response, ApiError> {
    let store = state.store;
    let job = tokio::task::spawn_blocking(move || store.load_job(&job_id))
        .await
        .map_err(|error| ApiError(DomainError::RouteJobStore(error.to_string())))??;
    match job {
        Some(job) => Ok((StatusCode::OK, Json(job)).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "route job not found"})),
        )
            .into_response()),
    }
}

async fn validate_polar(
    State(state): State<ApiState>,
    Json(input): Json<PolarValidationInput>,
) -> Result<Json<super::PolarValidationResult>, ApiError> {
    tokio::task::spawn_blocking(move || state.validate_polar(&input))
        .await
        .map_err(|error| ApiError(DomainError::PolarCache(error.to_string())))?
        .map(Json)
        .map_err(ApiError)
}

#[derive(Debug)]
struct ApiError(DomainError);

impl From<DomainError> for ApiError {
    fn from(error: DomainError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            DomainError::InvalidCoordinate { .. }
            | DomainError::InvalidRouteConfig(_)
            | DomainError::InvalidPolar(_) => StatusCode::BAD_REQUEST,
            DomainError::InvalidStartCoord
            | DomainError::NoSafeRouteFound
            | DomainError::WeatherOutOfBounds
            | DomainError::MissingWeatherData => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.0.to_string()}))).into_response()
    }
}
