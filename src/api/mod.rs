pub mod grpc_server;
pub mod nmea_reroute;
pub mod rest_gateway;

#[allow(clippy::result_large_err)]
pub mod routing {
    tonic::include_proto!("routing");
}

use std::sync::Arc;

use redis::{Client, Commands};
use serde::{Deserialize, Serialize};

use crate::core::isochrone::{
    ClimatologySource, RouteEngine, RouteRequest, RouteResult, RouteStatus, RoutingConfig,
    WeatherSource,
};
use crate::error::DomainError;
use crate::physics::polar::{CsvFormat, Polar};
use crate::spatial::geodesy::distance_and_bearing;
use crate::spatial::rtree_index::SpatialIndex;
use crate::types::GeoPoint;

pub const ASYNC_ROUTE_THRESHOLD_M: f64 = 200.0 * 1_852.0;
const JOB_TTL_SECONDS: u64 = 86_400;

#[derive(Clone)]
pub struct ApiState {
    pub engine: Arc<RouteEngine>,
    pub weather: Arc<dyn WeatherSource>,
    pub climatology: Option<Arc<dyn ClimatologySource>>,
    pub spatial: Arc<SpatialIndex>,
    pub store: Arc<dyn ApiStore>,
    pub routing_defaults: RoutingConfig,
}

pub trait ApiStore: Send + Sync {
    fn load_polar(&self, vessel_id: &str) -> Result<Option<Polar>, DomainError>;
    fn store_polar(&self, vessel_id: &str, polar: &Polar) -> Result<(), DomainError>;
    fn save_job(&self, job_id: &str, job: &RouteJob) -> Result<(), DomainError>;
    fn load_job(&self, job_id: &str) -> Result<Option<RouteJob>, DomainError>;
}

pub struct RedisApiStore {
    client: Client,
}

impl RedisApiStore {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl ApiStore for RedisApiStore {
    fn load_polar(&self, vessel_id: &str) -> Result<Option<Polar>, DomainError> {
        let mut connection = self
            .client
            .get_connection()
            .map_err(|error| DomainError::PolarCache(error.to_string()))?;
        Polar::load_from_redis(&mut connection, vessel_id)
    }

    fn store_polar(&self, vessel_id: &str, polar: &Polar) -> Result<(), DomainError> {
        let mut connection = self
            .client
            .get_connection()
            .map_err(|error| DomainError::PolarCache(error.to_string()))?;
        Polar::store_in_redis(&mut connection, vessel_id, polar)
    }

    fn save_job(&self, job_id: &str, job: &RouteJob) -> Result<(), DomainError> {
        let payload = serde_json::to_string(job)
            .map_err(|error| DomainError::RouteJobStore(error.to_string()))?;
        let mut connection = self
            .client
            .get_connection()
            .map_err(|error| DomainError::RouteJobStore(error.to_string()))?;
        connection
            .set_ex(job_key(job_id), payload, JOB_TTL_SECONDS)
            .map_err(|error| DomainError::RouteJobStore(error.to_string()))
    }

    fn load_job(&self, job_id: &str) -> Result<Option<RouteJob>, DomainError> {
        let mut connection = self
            .client
            .get_connection()
            .map_err(|error| DomainError::RouteJobStore(error.to_string()))?;
        let payload: Option<String> = connection
            .get(job_key(job_id))
            .map_err(|error| DomainError::RouteJobStore(error.to_string()))?;
        payload
            .map(|payload| {
                serde_json::from_str(&payload)
                    .map_err(|error| DomainError::RouteJobStore(error.to_string()))
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteJob {
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ApiRouteResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiRouteResult {
    pub status: String,
    pub waypoints: Vec<ApiWaypoint>,
    pub warnings: Vec<String>,
    pub isochrones_geojson_gzip_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiWaypoint {
    pub latitude: f64,
    pub longitude: f64,
    pub time_unix: i64,
    pub heading_degrees: f64,
    pub course_degrees: f64,
    pub sog_knots: f64,
    pub fuel_remaining_l: f64,
    pub sail_index: usize,
    pub is_motoring: bool,
    pub cumulative_cost_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinateInput {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateRouteInput {
    pub vessel_id: String,
    pub start: CoordinateInput,
    pub target: CoordinateInput,
    pub departure_time_unix: i64,
    #[serde(default)]
    pub initial_heading_degrees: f64,
    pub time_step_seconds: Option<i64>,
    pub heading_step_degrees: Option<f64>,
    pub max_tws_knots: Option<f64>,
    pub max_wave_height_m: Option<f64>,
    pub max_steps: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolarValidationInput {
    pub vessel_id: String,
    pub format: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolarValidationResult {
    pub valid: bool,
    pub maximum_speed_knots: f64,
    pub sail_count: usize,
}

impl ApiState {
    pub fn calculate(&self, input: &CalculateRouteInput) -> Result<RouteResult, DomainError> {
        let (request, config, polar) = self.prepare_route(input)?;
        self.engine.calculate_route(
            request,
            config,
            self.weather.as_ref(),
            self.climatology.as_deref(),
            &self.spatial,
            &polar,
        )
    }

    pub fn calculate_with_progress<F>(
        &self,
        input: &CalculateRouteInput,
        on_isochrone: F,
    ) -> Result<RouteResult, DomainError>
    where
        F: FnMut(&[GeoPoint], bool) + Send,
    {
        let (request, config, polar) = self.prepare_route(input)?;
        self.engine.calculate_route_with_progress(
            request,
            config,
            self.weather.as_ref(),
            self.climatology.as_deref(),
            &self.spatial,
            &polar,
            on_isochrone,
        )
    }

    fn prepare_route(
        &self,
        input: &CalculateRouteInput,
    ) -> Result<(RouteRequest, RoutingConfig, Polar), DomainError> {
        if input.vessel_id.trim().is_empty() {
            return Err(DomainError::InvalidPolar(
                "vessel_id must not be empty".to_owned(),
            ));
        }
        let start = GeoPoint::new(input.start.latitude, input.start.longitude)?;
        let target = GeoPoint::new(input.target.latitude, input.target.longitude)?;
        let mut config = self.routing_defaults;
        if let Some(time_step) = input.time_step_seconds {
            config.time_step_seconds = time_step;
        }
        if let Some(heading_step) = input.heading_step_degrees {
            config.heading_step_rad = heading_step.to_radians();
        }
        if let Some(max_tws) = input.max_tws_knots {
            config.max_tws_mps = max_tws * 1_852.0 / 3_600.0;
        }
        if let Some(max_wave_height) = input.max_wave_height_m {
            config.max_wave_height_m = max_wave_height;
        }
        if let Some(max_steps) = input.max_steps {
            config.max_steps = max_steps;
        }
        let route_request = RouteRequest {
            start,
            target,
            departure_time_unix: input.departure_time_unix,
            initial_heading_rad: input.initial_heading_degrees.to_radians(),
            initial_sail_index: 0,
        };
        let polar = self
            .store
            .load_polar(&input.vessel_id)?
            .ok_or_else(|| DomainError::InvalidPolar("no polar cached for vessel".to_owned()))?;
        Ok((route_request, config, polar))
    }

    pub fn route_distance_m(input: &CalculateRouteInput) -> Result<f64, DomainError> {
        let start = GeoPoint::new(input.start.latitude, input.start.longitude)?;
        let target = GeoPoint::new(input.target.latitude, input.target.longitude)?;
        Ok(distance_and_bearing(start, target)?.0)
    }

    pub fn validate_polar(
        &self,
        input: &PolarValidationInput,
    ) -> Result<PolarValidationResult, DomainError> {
        Polar::cache_key(&input.vessel_id)?;
        let polar = match input.format.as_str() {
            "orc_json" => Polar::parse_orc_json(&input.content)?,
            "csv_normalized" => Polar::parse_csv(&input.content, CsvFormat::Normalized)?,
            "csv_matrix" => Polar::parse_csv(&input.content, CsvFormat::Matrix)?,
            _ => {
                return Err(DomainError::InvalidPolar(
                    "format must be orc_json, csv_normalized, or csv_matrix".to_owned(),
                ));
            }
        };
        self.store.store_polar(&input.vessel_id, &polar)?;
        Ok(PolarValidationResult {
            valid: true,
            maximum_speed_knots: polar.maximum_speed_mps() * 3_600.0 / 1_852.0,
            sail_count: polar.sails().len(),
        })
    }
}

pub fn to_api_result(result: &RouteResult) -> Result<ApiRouteResult, DomainError> {
    let status = match result.status {
        RouteStatus::Completed => "COMPLETED",
        RouteStatus::NoSafeRouteFound => "NO_SAFE_ROUTE_FOUND",
    };
    let waypoints = result
        .waypoints
        .iter()
        .map(|waypoint| ApiWaypoint {
            latitude: waypoint.point.latitude(),
            longitude: waypoint.point.longitude(),
            time_unix: waypoint.time_unix,
            heading_degrees: waypoint.heading_rad.to_degrees().rem_euclid(360.0),
            course_degrees: waypoint.course_rad.to_degrees().rem_euclid(360.0),
            sog_knots: waypoint.sog_mps * 3_600.0 / 1_852.0,
            fuel_remaining_l: waypoint.fuel_remaining_l,
            sail_index: waypoint.sail_index,
            is_motoring: waypoint.is_motoring,
            cumulative_cost_s: waypoint.cumulative_cost_s,
        })
        .collect();
    let features: Vec<serde_json::Value> = result
        .partial_isochrones
        .iter()
        .enumerate()
        .map(|(index, isochrone)| {
            serde_json::json!({
                "type": "Feature",
                "properties": {"index": index},
                "geometry": {
                    "type": "MultiPoint",
                    "coordinates": isochrone.iter().map(|point| {
                        [point.longitude(), point.latitude()]
                    }).collect::<Vec<_>>()
                }
            })
        })
        .collect();
    let geojson = serde_json::json!({"type": "FeatureCollection", "features": features});
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    serde_json::to_writer(&mut encoder, &geojson)
        .map_err(|error| DomainError::RouteJobStore(error.to_string()))?;
    let compressed = encoder
        .finish()
        .map_err(|error| DomainError::RouteJobStore(error.to_string()))?;
    use base64::Engine;
    Ok(ApiRouteResult {
        status: status.to_owned(),
        waypoints,
        warnings: if result.warning_climatology_used {
            vec!["WARNING_CLIMATOLOGY_USED".to_owned()]
        } else {
            Vec::new()
        },
        isochrones_geojson_gzip_base64: base64::engine::general_purpose::STANDARD
            .encode(compressed),
    })
}

fn job_key(job_id: &str) -> String {
    format!("route-job:{job_id}")
}
