use std::f64::consts::TAU;

use rayon::ThreadPool;
use rayon::ThreadPoolBuilder;

use crate::core::cost::{evaluate_edge_cost, EdgeCostConfig, EdgeCostInput};
use crate::error::DomainError;
use crate::physics::kinematics::{
    compose_velocity_vectors, compute_true_wind, evaluate_motor_sailing, MotorSailingConfig,
};
use crate::physics::polar::Polar;
use crate::spatial::geodesy::{distance_and_bearing, project_point};
use crate::spatial::rtree_index::SpatialIndex;
use crate::types::{GeoPoint, WeatherSample};
use crate::weather::interpolator::WeatherGrid4D;

const BEARING_BUCKETS: usize = 360;
const SNAP_RADIUS_M: f64 = 3_704.0;
const MAX_HEADING_SAMPLES: usize = 36_000;

pub trait WeatherSource: Send + Sync {
    fn sample(&self, point: GeoPoint, time_unix: i64) -> Result<WeatherSample, DomainError>;

    fn forecast_bounds(&self) -> Option<(i64, i64)> {
        None
    }
}

pub trait ClimatologySource: Send + Sync {
    fn sample(&self, point: GeoPoint, time_unix: i64) -> Result<WeatherSample, DomainError>;
}

impl WeatherSource for WeatherGrid4D {
    fn sample(&self, point: GeoPoint, time_unix: i64) -> Result<WeatherSample, DomainError> {
        self.get_4d_point(point.latitude(), point.longitude(), time_unix)
    }

    fn forecast_bounds(&self) -> Option<(i64, i64)> {
        Some(self.time_bounds())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteStatus {
    Completed,
    NoSafeRouteFound,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waypoint {
    pub point: GeoPoint,
    pub time_unix: i64,
    pub heading_rad: f64,
    pub course_rad: f64,
    pub sog_mps: f64,
    pub fuel_remaining_l: f64,
    pub sail_index: usize,
    pub is_motoring: bool,
    pub cumulative_cost_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteResult {
    pub status: RouteStatus,
    pub waypoints: Vec<Waypoint>,
    pub partial_isochrones: Vec<Vec<GeoPoint>>,
    pub warning_climatology_used: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RoutingConfig {
    pub time_step_seconds: i64,
    pub heading_step_rad: f64,
    pub max_tws_mps: f64,
    pub max_wave_height_m: f64,
    pub max_steps: usize,
    pub leeway_coeff: f64,
    pub heel_rad: f64,
    pub initial_fuel_l: f64,
    pub motor_sailing: Option<MotorSailingConfig>,
    pub cost: EdgeCostConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct RouteRequest {
    pub start: GeoPoint,
    pub target: GeoPoint,
    pub departure_time_unix: i64,
    pub initial_heading_rad: f64,
    pub initial_sail_index: usize,
}

pub struct RouteEngine {
    pool: ThreadPool,
}

#[derive(Debug, Clone, Copy)]
struct SearchNode {
    point: GeoPoint,
    time_unix: i64,
    heading_rad: f64,
    course_rad: f64,
    sog_mps: f64,
    fuel_remaining_l: f64,
    sail_index: usize,
    is_motoring: bool,
    cumulative_cost_s: f64,
    parent: Option<usize>,
    twa_rad: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    point: GeoPoint,
    time_unix: i64,
    heading_rad: f64,
    course_rad: f64,
    sog_mps: f64,
    fuel_remaining_l: f64,
    sail_index: usize,
    is_motoring: bool,
    cumulative_cost_s: f64,
    parent: usize,
    twa_rad: f64,
}

#[derive(Debug, Clone, Copy)]
struct BucketCandidate {
    candidate: Candidate,
    heuristic_cost_s: f64,
}

impl RouteEngine {
    pub fn new(thread_count: usize) -> Result<Self, DomainError> {
        if thread_count == 0 {
            return Err(invalid_route_config("thread_count must be positive"));
        }
        let pool = ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .build()
            .map_err(|error| invalid_route_config(error.to_string()))?;
        Ok(Self { pool })
    }

    pub fn calculate_route<W: WeatherSource + ?Sized>(
        &self,
        request: RouteRequest,
        config: RoutingConfig,
        weather: &W,
        climatology: Option<&dyn ClimatologySource>,
        spatial: &SpatialIndex,
        polar: &Polar,
    ) -> Result<RouteResult, DomainError> {
        self.calculate_route_with_progress(
            request,
            config,
            weather,
            climatology,
            spatial,
            polar,
            |_, _| {},
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn calculate_route_with_progress<W, F>(
        &self,
        request: RouteRequest,
        config: RoutingConfig,
        weather: &W,
        climatology: Option<&dyn ClimatologySource>,
        spatial: &SpatialIndex,
        polar: &Polar,
        mut on_isochrone: F,
    ) -> Result<RouteResult, DomainError>
    where
        W: WeatherSource + ?Sized,
        F: FnMut(&[GeoPoint], bool) + Send,
    {
        self.pool.install(|| {
            calculate_serial(
                request,
                config,
                weather,
                climatology,
                spatial,
                polar,
                &mut on_isochrone,
            )
        })
    }
}

fn calculate_serial<W, F>(
    request: RouteRequest,
    config: RoutingConfig,
    weather: &W,
    climatology: Option<&dyn ClimatologySource>,
    spatial: &SpatialIndex,
    polar: &Polar,
    on_isochrone: &mut F,
) -> Result<RouteResult, DomainError>
where
    W: WeatherSource + ?Sized,
    F: FnMut(&[GeoPoint], bool),
{
    validate_request(request, config, polar)?;
    let start = spatial.snap_to_water(request.start, SNAP_RADIUS_M)?;
    let target = spatial.snap_to_water(request.target, SNAP_RADIUS_M)?;
    let max_sog = polar.maximum_speed_mps().max(
        config
            .motor_sailing
            .map_or(0.0, |motor| motor.motoring_sog_mps),
    );
    if max_sog <= 0.0 {
        return Err(invalid_route_config(
            "polar and motor configuration must provide positive maximum SOG",
        ));
    }
    let capture_radius_m = max_sog * config.time_step_seconds as f64;
    let initial_heading = request.initial_heading_rad.rem_euclid(TAU);
    let mut nodes = vec![SearchNode {
        point: start,
        time_unix: request.departure_time_unix,
        heading_rad: initial_heading,
        course_rad: initial_heading,
        sog_mps: 0.0,
        fuel_remaining_l: config.initial_fuel_l,
        sail_index: request.initial_sail_index,
        is_motoring: false,
        cumulative_cost_s: 0.0,
        parent: None,
        twa_rad: None,
    }];
    let mut frontier = vec![0usize];
    let mut partial_isochrones = vec![vec![start]];
    let mut warning_climatology_used = false;
    on_isochrone(&partial_isochrones[0], warning_climatology_used);

    let (initial_distance_m, _) = distance_and_bearing(start, target)?;
    if initial_distance_m == 0.0 {
        return Ok(RouteResult {
            status: RouteStatus::Completed,
            waypoints: vec![waypoint(nodes[0])],
            partial_isochrones,
            warning_climatology_used,
        });
    }

    for _ in 0..config.max_steps {
        let mut selected_by_bearing: [Option<BucketCandidate>; BEARING_BUCKETS] =
            [None; BEARING_BUCKETS];
        let mut final_candidate: Option<(Candidate, f64)> = None;
        for parent_index in &frontier {
            let parent = nodes[*parent_index];
            let (sample, used_climatology) =
                weather_at(weather, climatology, parent.point, parent.time_unix)?;
            warning_climatology_used |= used_climatology;
            let tws_mps = sample.wind_u.hypot(sample.wind_v);
            if tws_mps > config.max_tws_mps || sample.wave_height > config.max_wave_height_m {
                continue;
            }

            let heading_count = (TAU / config.heading_step_rad).ceil() as usize;
            for heading_index in 0..heading_count {
                let heading = (heading_index as f64 * config.heading_step_rad).min(TAU);
                if heading >= TAU {
                    continue;
                }
                let true_wind = compute_true_wind(sample.wind_u, sample.wind_v, heading)?;
                let performance = polar.best_speed(true_wind.angle_rad, true_wind.speed_mps)?;
                let (stw_mps, sail_id, sail_index) = match performance {
                    Some(performance) => {
                        let sail_index = polar
                            .sails()
                            .iter()
                            .position(|sail| sail.sail_id == performance.sail_id)
                            .ok_or_else(|| {
                                invalid_route_config("polar selected an unknown sail")
                            })?;
                        (performance.speed_mps, performance.sail_id, sail_index)
                    }
                    None if config.motor_sailing.is_some() => (
                        0.0,
                        polar.sails()[parent.sail_index].sail_id.as_str(),
                        parent.sail_index,
                    ),
                    None => continue,
                };
                let sailing_velocity = compose_velocity_vectors(
                    stw_mps,
                    heading,
                    sample.current_u,
                    sample.current_v,
                    config.leeway_coeff,
                    config.heel_rad,
                )?;
                let motor_outcome = if let Some(motor) = config.motor_sailing {
                    evaluate_motor_sailing(
                        sailing_velocity,
                        heading,
                        parent.fuel_remaining_l,
                        config.time_step_seconds as f64,
                        motor,
                    )?
                } else {
                    crate::physics::kinematics::MotorSailingOutcome {
                        velocity: sailing_velocity,
                        is_motoring: false,
                        fuel_remaining_l: parent.fuel_remaining_l,
                    }
                };
                let velocity = motor_outcome.velocity;
                if velocity.sog_mps <= 0.0 {
                    continue;
                }
                let next_point = project_point(
                    parent.point,
                    velocity.cog_rad,
                    velocity.sog_mps * config.time_step_seconds as f64,
                )?;
                if spatial.intersects_obstacle(parent.point, next_point) {
                    continue;
                }
                let next_time = parent
                    .time_unix
                    .checked_add(config.time_step_seconds)
                    .ok_or_else(|| invalid_route_config("route timestamp overflow"))?;
                let previous_twa = parent.twa_rad.unwrap_or(true_wind.angle_rad);
                let fuel_used = parent.fuel_remaining_l - motor_outcome.fuel_remaining_l;
                let edge_cost = evaluate_edge_cost(
                    EdgeCostInput {
                        elapsed_s: config.time_step_seconds as f64,
                        previous_twa_rad: previous_twa,
                        current_twa_rad: true_wind.angle_rad,
                        previous_sail: &polar.sails()[parent.sail_index].sail_id,
                        current_sail: sail_id,
                        fuel_used_l: fuel_used,
                        wave_height_m: sample.wave_height,
                    },
                    config.cost,
                )?;
                let candidate = Candidate {
                    point: next_point,
                    time_unix: next_time,
                    heading_rad: heading,
                    course_rad: velocity.cog_rad,
                    sog_mps: velocity.sog_mps,
                    fuel_remaining_l: motor_outcome.fuel_remaining_l,
                    sail_index,
                    is_motoring: motor_outcome.is_motoring,
                    cumulative_cost_s: parent.cumulative_cost_s + edge_cost.total_s,
                    parent: *parent_index,
                    twa_rad: true_wind.angle_rad,
                };
                let (distance_to_target_m, bearing_to_target_rad) =
                    distance_and_bearing(next_point, target)?;
                if distance_to_target_m <= capture_radius_m {
                    if spatial.intersects_obstacle(next_point, target) {
                        continue;
                    }
                    let remaining_seconds = distance_to_target_m / velocity.sog_mps;
                    let final_cost = candidate.cumulative_cost_s
                        + evaluate_edge_cost(
                            EdgeCostInput {
                                elapsed_s: remaining_seconds,
                                previous_twa_rad: true_wind.angle_rad,
                                current_twa_rad: true_wind.angle_rad,
                                previous_sail: sail_id,
                                current_sail: sail_id,
                                fuel_used_l: 0.0,
                                wave_height_m: sample.wave_height,
                            },
                            config.cost,
                        )?
                        .total_s;
                    if final_candidate.map_or(true, |(_, best_cost)| final_cost < best_cost) {
                        final_candidate = Some((candidate, final_cost));
                    }
                } else {
                    let bucket = bearing_bucket(bearing_to_target_rad);
                    let heuristic_cost_s =
                        candidate.cumulative_cost_s + distance_to_target_m / max_sog;
                    let existing = selected_by_bearing[bucket];
                    if existing.map_or(true, |current| heuristic_cost_s < current.heuristic_cost_s)
                    {
                        selected_by_bearing[bucket] = Some(BucketCandidate {
                            candidate,
                            heuristic_cost_s,
                        });
                    }
                }
            }
        }

        if let Some((candidate, final_cost)) = final_candidate {
            let candidate_index = nodes.len();
            nodes.push(candidate.into());
            let remaining_seconds =
                distance_and_bearing(candidate.point, target)?.0 / candidate.sog_mps;
            let final_time = candidate
                .time_unix
                .checked_add(remaining_seconds.ceil() as i64)
                .ok_or_else(|| invalid_route_config("route timestamp overflow"))?;
            nodes.push(SearchNode {
                point: target,
                time_unix: final_time,
                heading_rad: candidate.heading_rad,
                course_rad: candidate.course_rad,
                sog_mps: candidate.sog_mps,
                fuel_remaining_l: candidate.fuel_remaining_l,
                sail_index: candidate.sail_index,
                is_motoring: candidate.is_motoring,
                cumulative_cost_s: final_cost,
                parent: Some(candidate_index),
                twa_rad: Some(candidate.twa_rad),
            });
            let waypoints = backtrack(&nodes, nodes.len() - 1);
            let final_isochrone = waypoints
                .last()
                .map(|last| vec![last.point])
                .unwrap_or_default();
            on_isochrone(&final_isochrone, warning_climatology_used);
            partial_isochrones.push(final_isochrone);
            return Ok(RouteResult {
                status: RouteStatus::Completed,
                waypoints,
                partial_isochrones,
                warning_climatology_used,
            });
        }

        let mut next_frontier = Vec::with_capacity(BEARING_BUCKETS);
        for bucket_candidate in selected_by_bearing.into_iter().flatten() {
            let node_index = nodes.len();
            nodes.push(bucket_candidate.candidate.into());
            next_frontier.push(node_index);
        }
        if next_frontier.is_empty() {
            return Ok(RouteResult {
                status: RouteStatus::NoSafeRouteFound,
                waypoints: Vec::new(),
                partial_isochrones,
                warning_climatology_used,
            });
        }
        let isochrone: Vec<GeoPoint> = next_frontier
            .iter()
            .map(|index| nodes[*index].point)
            .collect();
        on_isochrone(&isochrone, warning_climatology_used);
        partial_isochrones.push(isochrone);
        frontier = next_frontier;
    }

    Ok(RouteResult {
        status: RouteStatus::NoSafeRouteFound,
        waypoints: Vec::new(),
        partial_isochrones,
        warning_climatology_used,
    })
}

impl From<Candidate> for SearchNode {
    fn from(candidate: Candidate) -> Self {
        Self {
            point: candidate.point,
            time_unix: candidate.time_unix,
            heading_rad: candidate.heading_rad,
            course_rad: candidate.course_rad,
            sog_mps: candidate.sog_mps,
            fuel_remaining_l: candidate.fuel_remaining_l,
            sail_index: candidate.sail_index,
            is_motoring: candidate.is_motoring,
            cumulative_cost_s: candidate.cumulative_cost_s,
            parent: Some(candidate.parent),
            twa_rad: Some(candidate.twa_rad),
        }
    }
}

fn validate_request(
    request: RouteRequest,
    config: RoutingConfig,
    polar: &Polar,
) -> Result<(), DomainError> {
    if config.time_step_seconds <= 0
        || !config.heading_step_rad.is_finite()
        || config.heading_step_rad <= 0.0
        || config.heading_step_rad > TAU
        || (TAU / config.heading_step_rad).ceil() > MAX_HEADING_SAMPLES as f64
        || !config.max_tws_mps.is_finite()
        || config.max_tws_mps < 0.0
        || !config.max_wave_height_m.is_finite()
        || config.max_wave_height_m < 0.0
        || config.max_steps == 0
        || !config.leeway_coeff.is_finite()
        || config.leeway_coeff < 0.0
        || !config.heel_rad.is_finite()
        || !config.initial_fuel_l.is_finite()
        || config.initial_fuel_l < 0.0
        || !request.initial_heading_rad.is_finite()
        || request.initial_sail_index >= polar.sails().len()
    {
        return Err(invalid_route_config(
            "routing request or limits are invalid",
        ));
    }
    Ok(())
}

fn weather_at<W: WeatherSource + ?Sized>(
    weather: &W,
    climatology: Option<&dyn ClimatologySource>,
    point: GeoPoint,
    time_unix: i64,
) -> Result<(WeatherSample, bool), DomainError> {
    if let Some((first, last)) = weather.forecast_bounds() {
        if time_unix < first || time_unix > last {
            if let Some(climatology) = climatology {
                let mut sample = climatology.sample(point, time_unix)?;
                sample.is_climatology = true;
                return Ok((sample, true));
            }
            return Err(DomainError::WeatherOutOfBounds);
        }
    }
    weather.sample(point, time_unix).map(|sample| {
        let used_climatology = sample.is_climatology;
        (sample, used_climatology)
    })
}

fn bearing_bucket(bearing_rad: f64) -> usize {
    (bearing_rad.to_degrees().floor() as usize) % BEARING_BUCKETS
}

fn waypoint(node: SearchNode) -> Waypoint {
    Waypoint {
        point: node.point,
        time_unix: node.time_unix,
        heading_rad: node.heading_rad,
        course_rad: node.course_rad,
        sog_mps: node.sog_mps,
        fuel_remaining_l: node.fuel_remaining_l,
        sail_index: node.sail_index,
        is_motoring: node.is_motoring,
        cumulative_cost_s: node.cumulative_cost_s,
    }
}

fn backtrack(nodes: &[SearchNode], mut index: usize) -> Vec<Waypoint> {
    let mut waypoints = Vec::new();
    loop {
        waypoints.push(waypoint(nodes[index]));
        match nodes[index].parent {
            Some(parent) => index = parent,
            None => break,
        }
    }
    waypoints.reverse();
    waypoints
}

fn invalid_route_config(message: impl Into<String>) -> DomainError {
    DomainError::InvalidRouteConfig(message.into())
}

#[cfg(test)]
mod tests {
    use super::{
        ClimatologySource, RouteEngine, RouteRequest, RouteStatus, RoutingConfig, WeatherSource,
    };
    use crate::core::cost::EdgeCostConfig;
    use crate::error::DomainError;
    use crate::physics::polar::Polar;
    use crate::spatial::rtree_index::SpatialIndex;
    use crate::types::{GeoPoint, WeatherSample};
    use geo::{Coord, LineString, Polygon};

    const WIND_KNOTS: f64 = 15.0;

    fn polar() -> Polar {
        Polar::parse_orc_json(
            r#"{
              "metadata":{"boatName":"Golden","unitTWS":"knots","unitBS":"knots"},
              "tws_grid":[15,20],
              "twa_grid":[0,42,90,138,180],
              "configurations":[
                {"sail_id":"main","matrix":[[0,0],[5,5],[6,6],[5,5],[4,4]]}
              ]
            }"#,
        )
        .unwrap()
    }

    fn upwind_polar() -> Polar {
        Polar::parse_orc_json(
            r#"{
              "metadata":{"boatName":"Golden","unitTWS":"knots","unitBS":"knots"},
              "tws_grid":[15,20],
              "twa_grid":[0,42,90,138,180],
              "configurations":[
                {"sail_id":"main","matrix":[[0,0],[5,5],[4,4],[5,5],[4,4]]}
              ]
            }"#,
        )
        .unwrap()
    }

    #[derive(Clone, Copy)]
    struct ConstantWeather {
        sample: WeatherSample,
        bounds: (i64, i64),
    }

    impl WeatherSource for ConstantWeather {
        fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
            Ok(self.sample)
        }

        fn forecast_bounds(&self) -> Option<(i64, i64)> {
            Some(self.bounds)
        }
    }

    struct ConstantClimatology(WeatherSample);

    impl ClimatologySource for ConstantClimatology {
        fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
            Ok(self.0)
        }
    }

    fn weather_from_north() -> ConstantWeather {
        ConstantWeather {
            sample: WeatherSample {
                wind_u: 0.0,
                wind_v: -WIND_KNOTS * 1_852.0 / 3_600.0,
                gust: 0.0,
                current_u: 0.0,
                current_v: 0.0,
                wave_height: 0.0,
                wave_period: 8.0,
                wave_dir: 0.0,
                is_climatology: false,
            },
            bounds: (0, 10_000_000),
        }
    }

    fn config() -> RoutingConfig {
        RoutingConfig {
            time_step_seconds: 3_600,
            heading_step_rad: 2.0_f64.to_radians(),
            max_tws_mps: 40.0 * 1_852.0 / 3_600.0,
            max_wave_height_m: 5.0,
            max_steps: 100,
            leeway_coeff: 0.0,
            heel_rad: 0.0,
            initial_fuel_l: 0.0,
            motor_sailing: None,
            cost: EdgeCostConfig {
                tack_penalty_s: 300.0,
                gybe_penalty_s: 180.0,
                sail_change_penalty_s: 60.0,
                fuel_weight_s_per_l: 0.0,
                wave_weight_s_per_m: 0.0,
                wave_comfort_height_m: 1.5,
            },
        }
    }

    #[test]
    fn rejects_heading_steps_that_would_exceed_the_sample_limit() {
        let engine = RouteEngine::new(1).unwrap();
        let error = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    GeoPoint::new(0.1, 0.0).unwrap(),
                ),
                RoutingConfig {
                    heading_step_rad: 1e-8,
                    ..config()
                },
                &weather_from_north(),
                None,
                &SpatialIndex::new([]),
                &upwind_polar(),
            )
            .unwrap_err();
        assert!(matches!(error, DomainError::InvalidRouteConfig(_)));
    }

    fn request(start: GeoPoint, target: GeoPoint) -> RouteRequest {
        RouteRequest {
            start,
            target,
            departure_time_unix: 0,
            initial_heading_rad: 0.0,
            initial_sail_index: 0,
        }
    }

    #[test]
    fn upwind_route_tacks_instead_of_sailing_into_the_wind() {
        let engine = RouteEngine::new(2).unwrap();
        let route = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    GeoPoint::new(1.0, 0.0).unwrap(),
                ),
                RoutingConfig {
                    heading_step_rad: 1.0_f64.to_radians(),
                    ..config()
                },
                &weather_from_north(),
                None,
                &SpatialIndex::new([]),
                &upwind_polar(),
            )
            .unwrap();

        assert_eq!(route.status, RouteStatus::Completed);
        assert_eq!(route.waypoints.last().unwrap().point.latitude(), 1.0);
        let headings: Vec<f64> = route
            .waypoints
            .iter()
            .map(|waypoint| waypoint.course_rad.to_degrees())
            .filter(|heading| (heading - 42.0).abs() <= 2.0 || (heading - 318.0).abs() <= 2.0)
            .collect();
        assert!(headings.len() > 2);
        assert!(headings
            .windows(2)
            .any(|pair| (pair[0] - pair[1]).abs() > 180.0));
        assert!(route.waypoints.iter().all(|waypoint| {
            let course = waypoint.course_rad.to_degrees();
            waypoint.sog_mps == 0.0 || (course - 42.0).abs() <= 2.0 || (course - 318.0).abs() <= 2.0
        }));
    }

    #[test]
    fn prunes_weather_blockade_and_keeps_partial_isochrones() {
        let engine = RouteEngine::new(1).unwrap();
        let route = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    GeoPoint::new(0.1, 0.0).unwrap(),
                ),
                RoutingConfig {
                    max_tws_mps: 5.0 * 1_852.0 / 3_600.0,
                    ..config()
                },
                &weather_from_north(),
                None,
                &SpatialIndex::new([]),
                &polar(),
            )
            .unwrap();

        assert_eq!(route.status, RouteStatus::NoSafeRouteFound);
        assert!(route.waypoints.is_empty());
        assert!(!route.partial_isochrones.is_empty());
    }

    #[test]
    fn uses_climatology_after_forecast_exhaustion_and_sets_warning() {
        let engine = RouteEngine::new(1).unwrap();
        let source = ConstantWeather {
            bounds: (0, 0),
            ..weather_from_north()
        };
        let route = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    GeoPoint::new(0.3, 0.0).unwrap(),
                ),
                config(),
                &source,
                Some(&ConstantClimatology(weather_from_north().sample)),
                &SpatialIndex::new([]),
                &polar(),
            )
            .unwrap();

        assert_eq!(route.status, RouteStatus::Completed);
        assert!(route.warning_climatology_used);
    }

    #[test]
    fn antimeridian_route_uses_short_eastward_path() {
        let engine = RouteEngine::new(1).unwrap();
        let mut weather = weather_from_north();
        weather.sample.wind_v = -12.0 * 1_852.0 / 3_600.0;
        let route = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 179.5).unwrap(),
                    GeoPoint::new(0.0, -179.5).unwrap(),
                ),
                config(),
                &weather,
                None,
                &SpatialIndex::new([]),
                &polar(),
            )
            .unwrap();

        assert_eq!(route.status, RouteStatus::Completed);
        let distance_m: f64 = route
            .waypoints
            .windows(2)
            .map(|pair| {
                crate::spatial::geodesy::distance_and_bearing(pair[0].point, pair[1].point)
                    .unwrap()
                    .0
            })
            .sum();
        assert!((distance_m - 60.0 * 1_852.0).abs() < 0.5 * 1_852.0);
        assert!(route
            .waypoints
            .windows(2)
            .any(|pair| { (pair[0].point.longitude() - pair[1].point.longitude()).abs() > 180.0 }));
    }

    #[test]
    fn avoids_an_island_and_prunes_a_total_storm_blockade() {
        let center_lat = 0.5_f64;
        let radius_deg = 10.0 / 60.0;
        let mut ring: Vec<Coord<f64>> = (0..72)
            .map(|index| {
                let angle = index as f64 * std::f64::consts::TAU / 72.0;
                Coord {
                    x: radius_deg * angle.cos() / center_lat.to_radians().cos(),
                    y: center_lat + radius_deg * angle.sin(),
                }
            })
            .collect();
        ring.push(ring[0]);
        let island = Polygon::new(LineString::new(ring), vec![]);
        let mut beam_wind = weather_from_north();
        beam_wind.sample.wind_u = WIND_KNOTS * 1_852.0 / 3_600.0;
        beam_wind.sample.wind_v = 0.0;
        let engine = RouteEngine::new(1).unwrap();
        let spatial = SpatialIndex::new([island]);
        let route = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    GeoPoint::new(1.0, 0.0).unwrap(),
                ),
                config(),
                &beam_wind,
                None,
                &spatial,
                &polar(),
            )
            .unwrap();
        assert_eq!(route.status, RouteStatus::Completed);
        assert!(route
            .waypoints
            .windows(2)
            .all(|pair| !spatial.intersects_obstacle(pair[0].point, pair[1].point)));

        let blockade_weather = ConstantWeather {
            sample: WeatherSample {
                wind_u: 0.0,
                wind_v: -60.0 * 1_852.0 / 3_600.0,
                gust: 0.0,
                current_u: 0.0,
                current_v: 0.0,
                wave_height: 0.0,
                wave_period: 8.0,
                wave_dir: 0.0,
                is_climatology: false,
            },
            bounds: (0, 10_000_000),
        };
        let blocked = engine
            .calculate_route(
                request(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    GeoPoint::new(0.1, 0.0).unwrap(),
                ),
                RoutingConfig {
                    max_tws_mps: 40.0 * 1_852.0 / 3_600.0,
                    ..config()
                },
                &blockade_weather,
                None,
                &SpatialIndex::new([]),
                &polar(),
            )
            .unwrap();
        assert_eq!(blocked.status, RouteStatus::NoSafeRouteFound);
        assert!(!blocked.partial_isochrones.is_empty());
    }
}
