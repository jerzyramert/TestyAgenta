use std::fs;
use std::path::Path;
use std::time::Instant;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use geo::{LineString, Polygon};
use sailboat_routing_engine::core::cost::EdgeCostConfig;
use sailboat_routing_engine::core::isochrone::{
    RouteEngine, RouteRequest, RoutingConfig, WeatherSource,
};
use sailboat_routing_engine::error::DomainError;
use sailboat_routing_engine::physics::polar::Polar;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;
use sailboat_routing_engine::types::{GeoPoint, WeatherSample};
use sailboat_routing_engine::weather::interpolator::{WeatherGrid4D, WeatherVariables};
use tempfile::TempDir;

struct ConstantWeather(WeatherSample);

impl WeatherSource for ConstantWeather {
    fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
        Ok(self.0)
    }

    fn forecast_bounds(&self) -> Option<(i64, i64)> {
        Some((0, 10_000_000))
    }
}

fn point(latitude: f64, longitude: f64) -> GeoPoint {
    GeoPoint::new(latitude, longitude).expect("benchmark coordinates are valid")
}

fn weather_grid_fixture() -> (TempDir, WeatherGrid4D) {
    let directory = tempfile::tempdir().expect("create temporary Zarr store");
    let store = directory.path();
    write_array(
        &store.join("lat"),
        &[2],
        &[2],
        "<f8",
        &["lat"],
        None,
        &[-1.0_f64, 1.0]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    write_array(
        &store.join("lon"),
        &[2],
        &[2],
        "<f8",
        &["lon"],
        None,
        &[-1.0_f64, 1.0]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    write_array(
        &store.join("time"),
        &[2],
        &[2],
        "<i8",
        &["time"],
        None,
        &[0_i64, 10_000_000]
            .into_iter()
            .flat_map(i64::to_le_bytes)
            .collect::<Vec<_>>(),
    );

    let names = [
        "wind_u",
        "wind_v",
        "gust",
        "current_u",
        "current_v",
        "wave_height",
        "wave_period",
        "wave_dir",
    ];
    let units = ["m s-1", "m s-1", "m s-1", "m s-1", "m s-1", "m", "s", "rad"];
    for (name, unit) in names.into_iter().zip(units) {
        let value = 1.0_f64;
        let bytes: Vec<u8> = std::iter::repeat(value.to_le_bytes())
            .take(8)
            .flatten()
            .collect();
        write_array(
            &store.join(name),
            &[2, 2, 2],
            &[2, 2, 2],
            "<f8",
            &["lat", "lon", "time"],
            Some(unit),
            &bytes,
        );
    }

    let variables = WeatherVariables::new(
        "wind_u",
        "wind_v",
        "gust",
        "current_u",
        "current_v",
        "wave_height",
        "wave_period",
        "wave_dir",
    );
    let grid = WeatherGrid4D::open_zarr(store, &variables).expect("open fixture grid");
    (directory, grid)
}

fn write_array(
    path: &Path,
    shape: &[usize],
    chunks: &[usize],
    dtype: &str,
    dimensions: &[&str],
    units: Option<&str>,
    bytes: &[u8],
) {
    fs::create_dir(path).expect("create Zarr array directory");
    fs::write(
        path.join(".zarray"),
        serde_json::to_vec(&serde_json::json!({
            "zarr_format": 2,
            "shape": shape,
            "chunks": chunks,
            "dtype": dtype,
            "compressor": null,
            "fill_value": "NaN",
            "order": "C",
            "filters": null,
            "dimension_separator": "."
        }))
        .expect("serialize Zarr metadata"),
    )
    .expect("write Zarr metadata");
    let attributes = match units {
        Some(units) => serde_json::json!({"_ARRAY_DIMENSIONS": dimensions, "units": units}),
        None => serde_json::json!({"_ARRAY_DIMENSIONS": dimensions}),
    };
    fs::write(
        path.join(".zattrs"),
        serde_json::to_vec(&attributes).expect("serialize Zarr attributes"),
    )
    .expect("write Zarr attributes");
    fs::write(path.join(vec!["0"; shape.len()].join(".")), bytes).expect("write Zarr chunk");
}

fn routing_fixture() -> (
    RouteEngine,
    RoutingConfig,
    ConstantWeather,
    SpatialIndex,
    Polar,
) {
    let engine = RouteEngine::new(1).expect("create route engine");
    let weather = ConstantWeather(WeatherSample {
        wind_u: 0.0,
        wind_v: -8.0,
        gust: 0.0,
        current_u: 0.0,
        current_v: 0.0,
        wave_height: 0.0,
        wave_period: 8.0,
        wave_dir: 0.0,
        is_climatology: false,
    });
    let spatial = SpatialIndex::new([]);
    let polar = Polar::parse_orc_json(
        r#"{
          "metadata":{"boatName":"Bench","unitTWS":"knots","unitBS":"knots"},
          "tws_grid":[10,20,30],
          "twa_grid":[0,45,90,135,180],
          "configurations":[
            {"sail_id":"main","matrix":[[0,0,0],[4,5,6],[5,6,7],[4,5,6],[3,4,5]]}
          ]
        }"#,
    )
    .expect("parse benchmark polar");
    let config = RoutingConfig {
        time_step_seconds: 3_600,
        heading_step_rad: 30.0_f64.to_radians(),
        max_tws_mps: 20.0,
        max_wave_height_m: 5.0,
        max_steps: 1_000,
        leeway_coeff: 0.0,
        heel_rad: 0.0,
        initial_fuel_l: 0.0,
        motor_sailing: None,
        cost: EdgeCostConfig {
            tack_penalty_s: 0.0,
            gybe_penalty_s: 0.0,
            sail_change_penalty_s: 0.0,
            fuel_weight_s_per_l: 0.0,
            wave_weight_s_per_m: 0.0,
            wave_comfort_height_m: 2.0,
        },
    };
    (engine, config, weather, spatial, polar)
}

fn collision_index() -> SpatialIndex {
    let mut obstacles = vec![Polygon::new(
        LineString::from(vec![
            (0.04, 0.04),
            (0.06, 0.04),
            (0.06, 0.06),
            (0.04, 0.06),
            (0.04, 0.04),
        ]),
        vec![],
    )];
    obstacles.extend((0..1_000).map(|index| {
        let longitude = 10.0 + index as f64 * 0.01;
        Polygon::new(
            LineString::from(vec![
                (longitude, 10.0),
                (longitude + 0.001, 10.0),
                (longitude + 0.001, 10.001),
                (longitude, 10.001),
                (longitude, 10.0),
            ]),
            vec![],
        )
    }));
    SpatialIndex::new(obstacles)
}

fn run_route(
    fixture: &(
        RouteEngine,
        RoutingConfig,
        ConstantWeather,
        SpatialIndex,
        Polar,
    ),
    start: GeoPoint,
    target: GeoPoint,
) {
    let (engine, config, weather, spatial, polar) = fixture;
    let route = engine
        .calculate_route(
            RouteRequest {
                start,
                target,
                departure_time_unix: 0,
                initial_heading_rad: 90.0_f64.to_radians(),
                initial_sail_index: 0,
            },
            *config,
            weather,
            None,
            spatial,
            polar,
        )
        .expect("benchmark route calculation succeeds");
    assert!(!route.waypoints.is_empty(), "benchmark route has waypoints");
}

fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // getrusage initializes the struct on success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    let peak = unsafe { usage.assume_init() }.ru_maxrss as u64;
    #[cfg(target_os = "macos")]
    {
        Some(peak)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(peak * 1024)
    }
}

fn routing_benchmarks(criterion: &mut Criterion) {
    let (_grid_directory, grid) = weather_grid_fixture();
    let mut interpolation = criterion.benchmark_group("weather_interpolation");
    interpolation.throughput(Throughput::Elements(100_000));
    interpolation.bench_function("100000_queries", |bencher| {
        bencher.iter(|| {
            for index in 0..100_000 {
                let offset = (index % 100) as f64 / 100.0;
                black_box(
                    grid.get_4d_point(-0.5 + offset, -0.5 + offset, 5_000_000)
                        .expect("interpolation query is in bounds"),
                );
            }
        });
    });
    interpolation.finish();

    let spatial = collision_index();
    let start = point(0.0, 0.0);
    let end = point(0.1, 0.1);
    let mut collisions = criterion.benchmark_group("rtree_collision");
    collisions.throughput(Throughput::Elements(100_000));
    collisions.bench_function("100000_segment_checks", |bencher| {
        bencher.iter(|| {
            for _ in 0..100_000 {
                black_box(spatial.intersects_obstacle(start, end));
            }
        });
    });
    collisions.finish();

    let fixture = routing_fixture();
    let coastal_start = point(0.0, 0.0);
    let coastal_target = point(0.0, 2.0);
    let atlantic_start = point(40.0, -70.0);
    let atlantic_target = point(0.0, -20.0);
    let profile_started = Instant::now();
    run_route(&fixture, atlantic_start, atlantic_target);
    eprintln!(
        "Transatlantic profile: elapsed={:?}, peak_rss_bytes={:?}",
        profile_started.elapsed(),
        peak_rss_bytes()
    );

    let mut routes = criterion.benchmark_group("end_to_end_routing");
    routes.bench_with_input(
        BenchmarkId::new("coastal_under_200_nm", "120_nm"),
        &(coastal_start, coastal_target),
        |bencher, &(from, to)| bencher.iter(|| run_route(&fixture, from, to)),
    );
    routes.bench_with_input(
        BenchmarkId::new("transatlantic_3000_nm", "3600_second_step"),
        &(atlantic_start, atlantic_target),
        |bencher, &(from, to)| bencher.iter(|| run_route(&fixture, from, to)),
    );
    routes.finish();
}

criterion_group!(benches, routing_benchmarks);
criterion_main!(benches);
