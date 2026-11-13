use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sailboat_routing_engine::api::grpc_server::grpc_service;
use sailboat_routing_engine::api::rest_gateway;
use sailboat_routing_engine::api::{ApiState, RedisApiStore};
use sailboat_routing_engine::core::cost::EdgeCostConfig;
use sailboat_routing_engine::core::isochrone::{RouteEngine, RoutingConfig};
use sailboat_routing_engine::physics::kinematics::MotorSailingConfig;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;
use sailboat_routing_engine::weather::interpolator::{WeatherGrid4D, WeatherVariables};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let redis_url = required("REDIS_URL")?;
    let weather_path = required("WEATHER_ZARR_PATH")?;
    let postgis_url = required("POSTGIS_DATABASE_URL")?;
    let weather_variables = WeatherVariables::new(
        variable("WEATHER_WIND_U", "wind_u"),
        variable("WEATHER_WIND_V", "wind_v"),
        variable("WEATHER_GUST", "gust"),
        variable("WEATHER_CURRENT_U", "current_u"),
        variable("WEATHER_CURRENT_V", "current_v"),
        variable("WEATHER_WAVE_HEIGHT", "wave_height"),
        variable("WEATHER_WAVE_PERIOD", "wave_period"),
        variable("WEATHER_WAVE_DIRECTION", "wave_dir"),
    );
    let weather = Arc::new(WeatherGrid4D::open_zarr(weather_path, &weather_variables)?);
    let valid_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let spatial = Arc::new(SpatialIndex::load_postgis(&postgis_url, valid_at)?);
    let redis_client = redis::Client::open(redis_url)?;
    let thread_count = std::thread::available_parallelism()?.get();
    let engine = Arc::new(RouteEngine::new(thread_count)?);
    let state = ApiState {
        engine,
        weather,
        climatology: None,
        spatial,
        store: Arc::new(RedisApiStore::new(redis_client)),
        routing_defaults: routing_config_from_env()?,
    };

    let rest_address: SocketAddr = variable("REST_ADDR", "0.0.0.0:8080").parse()?;
    let grpc_address: SocketAddr = variable("GRPC_ADDR", "0.0.0.0:50051").parse()?;
    let rest_listener = tokio::net::TcpListener::bind(rest_address).await?;
    let rest_state = state.clone();
    let grpc_state = state;
    tokio::try_join!(
        async move {
            axum::serve(rest_listener, rest_gateway::router(rest_state))
                .await
                .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)
        },
        async move {
            tonic::transport::Server::builder()
                .add_service(grpc_service(grpc_state))
                .serve(grpc_address)
                .await
                .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)
        }
    )?;
    Ok(())
}

fn routing_config_from_env() -> Result<RoutingConfig, Box<dyn Error + Send + Sync>> {
    Ok(RoutingConfig {
        time_step_seconds: required("ROUTING_TIME_STEP_SECONDS")?.parse()?,
        heading_step_rad: required("ROUTING_HEADING_STEP_DEGREES")?
            .parse::<f64>()?
            .to_radians(),
        max_tws_mps: required("ROUTING_MAX_TWS_KNOTS")?.parse::<f64>()? * 1_852.0 / 3_600.0,
        max_wave_height_m: required("ROUTING_MAX_WAVE_HEIGHT_M")?.parse()?,
        max_steps: required("ROUTING_MAX_STEPS")?.parse()?,
        leeway_coeff: required("ROUTING_LEEWAY_COEFF")?.parse()?,
        heel_rad: required("ROUTING_HEEL_DEGREES")?
            .parse::<f64>()?
            .to_radians(),
        initial_fuel_l: required("ROUTING_INITIAL_FUEL_L")?.parse()?,
        motor_sailing: motor_sailing_from_env()?,
        cost: EdgeCostConfig {
            tack_penalty_s: required("ROUTING_TACK_PENALTY_S")?.parse()?,
            gybe_penalty_s: required("ROUTING_GYBE_PENALTY_S")?.parse()?,
            sail_change_penalty_s: required("ROUTING_SAIL_CHANGE_PENALTY_S")?.parse()?,
            fuel_weight_s_per_l: required("ROUTING_FUEL_WEIGHT_S_PER_L")?.parse()?,
            wave_weight_s_per_m: required("ROUTING_WAVE_WEIGHT_S_PER_M")?.parse()?,
            wave_comfort_height_m: required("ROUTING_WAVE_COMFORT_HEIGHT_M")?.parse()?,
        },
    })
}

fn motor_sailing_from_env() -> Result<Option<MotorSailingConfig>, Box<dyn Error + Send + Sync>> {
    let min_sog = optional("ROUTING_MIN_MOTORING_SOG_KNOTS")?;
    let motoring_sog = optional("ROUTING_MOTORING_SOG_KNOTS")?;
    let fuel_rate = optional("ROUTING_MOTOR_FUEL_RATE_LPH")?;

    parse_motor_sailing_config(
        min_sog.as_deref(),
        motoring_sog.as_deref(),
        fuel_rate.as_deref(),
    )
}

fn parse_motor_sailing_config(
    min_sog: Option<&str>,
    motoring_sog: Option<&str>,
    fuel_rate: Option<&str>,
) -> Result<Option<MotorSailingConfig>, Box<dyn Error + Send + Sync>> {
    match (min_sog, motoring_sog, fuel_rate) {
        (None, None, None) => Ok(None),
        (Some(min_sog), Some(motoring_sog), Some(fuel_rate)) => {
            let knots_to_mps = 1_852.0 / 3_600.0;
            Ok(Some(MotorSailingConfig {
                min_motoring_sog_mps: min_sog.parse::<f64>()? * knots_to_mps,
                motoring_sog_mps: motoring_sog.parse::<f64>()? * knots_to_mps,
                fuel_rate_lph: fuel_rate.parse()?,
            }))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ROUTING_MIN_MOTORING_SOG_KNOTS, ROUTING_MOTORING_SOG_KNOTS, and ROUTING_MOTOR_FUEL_RATE_LPH must be set together",
        )
        .into()),
    }
}

fn optional(name: &str) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(Box::new(error)),
    }
}

fn required(name: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    std::env::var(name)
        .map_err(|_| format!("required environment variable {name} is missing").into())
}

fn variable(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[cfg(test)]
mod tests {
    use super::parse_motor_sailing_config;
    use sailboat_routing_engine::physics::kinematics::MotorSailingConfig;

    #[test]
    fn parses_optional_motor_sailing_settings_in_si_units() {
        assert_eq!(parse_motor_sailing_config(None, None, None).unwrap(), None);
        assert_eq!(
            parse_motor_sailing_config(Some("2"), Some("5"), Some("1")).unwrap(),
            Some(MotorSailingConfig {
                min_motoring_sog_mps: 2.0 * 1_852.0 / 3_600.0,
                motoring_sog_mps: 5.0 * 1_852.0 / 3_600.0,
                fuel_rate_lph: 1.0,
            })
        );
        assert!(parse_motor_sailing_config(Some("2"), None, Some("1")).is_err());
    }
}
