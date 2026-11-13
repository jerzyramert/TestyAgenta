use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use super::routing::route_engine_server::RouteEngine;
use super::routing::{
    Coordinate, PolarValidationRequest, PolarValidationResponse, RouteEvent, RouteRequest,
    RouteResponse,
};
use super::{ApiState, CalculateRouteInput, CoordinateInput, PolarValidationInput};
use crate::core::isochrone::{RouteResult, Waypoint};
use crate::error::DomainError;

#[derive(Clone)]
pub struct GrpcRouteEngine {
    state: ApiState,
}

impl GrpcRouteEngine {
    pub fn new(state: ApiState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl RouteEngine for GrpcRouteEngine {
    type CalculateRouteStream =
        Pin<Box<dyn Stream<Item = Result<RouteResponse, Status>> + Send + 'static>>;

    async fn calculate_route(
        &self,
        request: Request<RouteRequest>,
    ) -> Result<Response<Self::CalculateRouteStream>, Status> {
        let input = route_input(request.into_inner()).map_err(|status| *status)?;
        let state = self.state.clone();
        let (sender, receiver) = mpsc::channel(8);
        let progress_sender = sender.clone();
        let _worker = tokio::task::spawn_blocking(move || {
            let mut index = 0_u32;
            let route = state.calculate_with_progress(&input, |points, warning| {
                let message = RouteResponse {
                    event: RouteEvent::IsochroneChunk as i32,
                    isochrone_index: index,
                    isochrone_points: points.iter().copied().map(coordinate).collect(),
                    waypoints: Vec::new(),
                    status: "PROCESSING".to_owned(),
                    warning_climatology_used: warning,
                };
                index = index.saturating_add(1);
                let _ = progress_sender.blocking_send(Ok(message));
            });
            match route {
                Ok(route) => {
                    let _ = sender.blocking_send(Ok(final_route_response(route)));
                }
                Err(error) => {
                    let _ = sender.blocking_send(Err(domain_status(error)));
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }

    async fn validate_polar(
        &self,
        request: Request<PolarValidationRequest>,
    ) -> Result<Response<PolarValidationResponse>, Status> {
        let request = request.into_inner();
        let input = PolarValidationInput {
            vessel_id: request.vessel_id,
            format: request.format,
            content: request.content,
        };
        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || state.validate_polar(&input))
            .await
            .map_err(|error| Status::internal(error.to_string()))?
            .map_err(domain_status)?;
        Ok(Response::new(PolarValidationResponse {
            valid: result.valid,
            maximum_speed_knots: result.maximum_speed_knots,
            sail_count: result.sail_count as u32,
        }))
    }
}

fn route_input(request: RouteRequest) -> Result<CalculateRouteInput, Box<Status>> {
    let start = request
        .start
        .ok_or_else(|| Box::new(Status::invalid_argument("start coordinate is required")))?;
    let target = request
        .target
        .ok_or_else(|| Box::new(Status::invalid_argument("target coordinate is required")))?;
    Ok(CalculateRouteInput {
        vessel_id: request.vessel_id,
        start: CoordinateInput {
            latitude: start.latitude,
            longitude: start.longitude,
        },
        target: CoordinateInput {
            latitude: target.latitude,
            longitude: target.longitude,
        },
        departure_time_unix: request.departure_time_unix,
        initial_heading_degrees: request.initial_heading_degrees,
        time_step_seconds: request.time_step_seconds,
        heading_step_degrees: request.heading_step_degrees,
        max_tws_knots: request.max_tws_knots,
        max_wave_height_m: request.max_wave_height_m,
        max_steps: request.max_steps.map(|steps| steps as usize),
    })
}

fn final_route_response(route: RouteResult) -> RouteResponse {
    let status = match route.status {
        crate::core::isochrone::RouteStatus::Completed => "COMPLETED",
        crate::core::isochrone::RouteStatus::NoSafeRouteFound => "NO_SAFE_ROUTE_FOUND",
    }
    .to_owned();
    let waypoints = route
        .waypoints
        .iter()
        .copied()
        .map(route_waypoint)
        .collect();
    RouteResponse {
        event: RouteEvent::FinalRoute as i32,
        isochrone_index: route.partial_isochrones.len() as u32,
        isochrone_points: Vec::new(),
        waypoints,
        status,
        warning_climatology_used: route.warning_climatology_used,
    }
}

fn coordinate(point: crate::types::GeoPoint) -> Coordinate {
    Coordinate {
        latitude: point.latitude(),
        longitude: point.longitude(),
    }
}

fn route_waypoint(waypoint: Waypoint) -> super::routing::Waypoint {
    super::routing::Waypoint {
        point: Some(coordinate(waypoint.point)),
        time_unix: waypoint.time_unix,
        heading_degrees: waypoint.heading_rad.to_degrees().rem_euclid(360.0),
        course_degrees: waypoint.course_rad.to_degrees().rem_euclid(360.0),
        sog_knots: waypoint.sog_mps * 3_600.0 / 1_852.0,
        fuel_remaining_l: waypoint.fuel_remaining_l,
        sail_index: waypoint.sail_index as u32,
        is_motoring: waypoint.is_motoring,
        cumulative_cost_s: waypoint.cumulative_cost_s,
    }
}

fn domain_status(error: DomainError) -> Status {
    match &error {
        DomainError::InvalidCoordinate { .. }
        | DomainError::InvalidRouteConfig(_)
        | DomainError::InvalidPolar(_) => Status::invalid_argument(error.to_string()),
        DomainError::InvalidStartCoord
        | DomainError::NoSafeRouteFound
        | DomainError::WeatherOutOfBounds
        | DomainError::MissingWeatherData => Status::failed_precondition(error.to_string()),
        _ => Status::internal(error.to_string()),
    }
}

pub fn grpc_service(
    state: ApiState,
) -> super::routing::route_engine_server::RouteEngineServer<GrpcRouteEngine> {
    super::routing::route_engine_server::RouteEngineServer::new(GrpcRouteEngine::new(state))
}
