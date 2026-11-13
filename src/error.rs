use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum DomainError {
    #[error("ERR_INVALID_START_COORD: no navigable water found within the snap tolerance")]
    InvalidStartCoord,
    #[error("ERR_NO_SAFE_ROUTE_FOUND: all candidate route branches were pruned")]
    NoSafeRouteFound,
    #[error("invalid geographic coordinate: latitude {latitude}, longitude {longitude}")]
    InvalidCoordinate { latitude: f64, longitude: f64 },
    #[error("geodesic distance must be finite and non-negative")]
    InvalidDistance,
    #[error("geodesic bearing must be finite")]
    InvalidBearing,
    #[error("geodesic calculation did not converge")]
    GeodesyDidNotConverge,
    #[error("snap radius must be finite and non-negative")]
    InvalidSnapRadius,
    #[error("PostGIS operation failed: {0}")]
    Postgis(String),
    #[error("invalid PostGIS geometry: {0}")]
    InvalidPostgisGeometry(String),
    #[error("invalid or unsupported Zarr array: {0}")]
    InvalidZarr(String),
    #[error("weather query is outside the available grid or forecast horizon")]
    WeatherOutOfBounds,
    #[error("weather query overlaps a missing or non-finite grid value")]
    MissingWeatherData,
    #[error("invalid vessel polar: {0}")]
    InvalidPolar(String),
    #[error("Redis polar cache operation failed: {0}")]
    PolarCache(String),
    #[error("invalid vessel kinematics input: {0}")]
    InvalidKinematics(String),
    #[error("invalid nested-weather blend input: {0}")]
    InvalidWeatherBlend(String),
    #[error("invalid routing cost input: {0}")]
    InvalidCostInput(String),
    #[error("invalid ensemble routing input: {0}")]
    InvalidEnsembleInput(String),
    #[error("invalid NMEA 2000 frame: {0}")]
    InvalidNmeaFrame(String),
    #[error("unsupported NMEA 2000 PGN: {0}")]
    UnsupportedNmeaPgn(u32),
    #[error("invalid route configuration: {0}")]
    InvalidRouteConfig(String),
    #[error("route job storage operation failed: {0}")]
    RouteJobStore(String),
}
