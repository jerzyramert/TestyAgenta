use geo::{Coord, LineString, Polygon};
use sailboat_routing_engine::core::cost::EdgeCostConfig;
use sailboat_routing_engine::core::isochrone::{
    ClimatologySource, RouteEngine, RouteRequest, RoutingConfig, WeatherSource,
};
use sailboat_routing_engine::error::DomainError;
use sailboat_routing_engine::physics::kinematics::MotorSailingConfig;
use sailboat_routing_engine::physics::polar::Polar;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;
use sailboat_routing_engine::types::{GeoPoint, WeatherSample};

pub const KNOT_TO_MPS: f64 = 1_852.0 / 3_600.0;
pub const NAUTICAL_MILE_M: f64 = 1_852.0;
const WIND_KNOTS: f64 = 15.0;

pub struct ConstantWeather {
    pub sample: WeatherSample,
    pub bounds: Option<(i64, i64)>,
}

impl WeatherSource for ConstantWeather {
    fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
        Ok(self.sample)
    }

    fn forecast_bounds(&self) -> Option<(i64, i64)> {
        self.bounds
    }
}

pub struct ConstantClimatology(pub WeatherSample);

impl ClimatologySource for ConstantClimatology {
    fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
        Ok(self.0)
    }
}

pub fn point(latitude: f64, longitude: f64) -> GeoPoint {
    GeoPoint::new(latitude, longitude).expect("golden-case coordinates are valid")
}

pub fn polar() -> Polar {
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
    .expect("golden-case polar is valid")
}

pub fn upwind_polar() -> Polar {
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
    .expect("upwind golden-case polar is valid")
}

pub fn weather(wind_u_knots: f64, wind_v_knots: f64) -> ConstantWeather {
    ConstantWeather {
        sample: WeatherSample {
            wind_u: wind_u_knots * KNOT_TO_MPS,
            wind_v: wind_v_knots * KNOT_TO_MPS,
            gust: 0.0,
            current_u: 0.0,
            current_v: 0.0,
            wave_height: 0.0,
            wave_period: 8.0,
            wave_dir: 0.0,
            is_climatology: false,
        },
        bounds: Some((0, 10_000_000)),
    }
}

pub fn north_wind() -> ConstantWeather {
    weather(0.0, -WIND_KNOTS)
}

pub fn config() -> RoutingConfig {
    RoutingConfig {
        time_step_seconds: 3_600,
        heading_step_rad: 2.0_f64.to_radians(),
        max_tws_mps: 40.0 * KNOT_TO_MPS,
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

pub fn request(start: GeoPoint, target: GeoPoint) -> RouteRequest {
    RouteRequest {
        start,
        target,
        departure_time_unix: 0,
        initial_heading_rad: 0.0,
        initial_sail_index: 0,
    }
}

pub fn engine() -> RouteEngine {
    RouteEngine::new(2).expect("golden-case route engine starts")
}

pub fn circular_obstacle(center_lat: f64, center_lon: f64, radius_m: f64) -> Polygon<f64> {
    let radius_lat_deg = radius_m / 111_320.0;
    let radius_lon_deg = radius_lat_deg / center_lat.to_radians().cos();
    let mut ring: Vec<Coord<f64>> = (0..72)
        .map(|index| {
            let angle = index as f64 * std::f64::consts::TAU / 72.0;
            Coord {
                x: center_lon + radius_lon_deg * angle.cos(),
                y: center_lat + radius_lat_deg * angle.sin(),
            }
        })
        .collect();
    ring.push(ring[0]);
    Polygon::new(LineString::new(ring), vec![])
}

pub fn motor_config() -> MotorSailingConfig {
    MotorSailingConfig {
        min_motoring_sog_mps: 2.0 * KNOT_TO_MPS,
        motoring_sog_mps: 5.0 * KNOT_TO_MPS,
        fuel_rate_lph: 1.0,
    }
}

pub fn empty_spatial_index() -> SpatialIndex {
    SpatialIndex::new([])
}
