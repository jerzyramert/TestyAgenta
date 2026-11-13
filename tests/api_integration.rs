use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use flate2::read::GzDecoder;
use redis::Commands;
use sailboat_routing_engine::api::grpc_server::grpc_service;
use sailboat_routing_engine::api::rest_gateway;
use sailboat_routing_engine::api::routing::route_engine_client::RouteEngineClient;
use sailboat_routing_engine::api::routing::{
    Coordinate, PolarValidationRequest, RouteEvent, RouteRequest,
};
use sailboat_routing_engine::api::{
    ApiState, ApiStore, CalculateRouteInput, CoordinateInput, RouteJob,
};
use sailboat_routing_engine::core::cost::EdgeCostConfig;
use sailboat_routing_engine::core::isochrone::{
    ClimatologySource, RouteEngine, RoutingConfig, WeatherSource,
};
use sailboat_routing_engine::error::DomainError;
use sailboat_routing_engine::physics::polar::Polar;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;
use sailboat_routing_engine::types::{GeoPoint, WeatherSample};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_stream::wrappers::TcpListenerStream;

const POLAR_JSON: &str = r#"{
  "metadata":{"boatName":"API test vessel","unitTWS":"knots","unitBS":"knots"},
  "tws_grid":[15,20],
  "twa_grid":[0,42,90,138,180],
  "configurations":[
    {"sail_id":"main","matrix":[[0,0],[5,5],[6,6],[5,5],[4,4]]}
  ]
}"#;

#[derive(Default)]
struct MemoryStore {
    polars: Mutex<HashMap<String, Polar>>,
    jobs: Mutex<HashMap<String, RouteJob>>,
}

impl ApiStore for MemoryStore {
    fn load_polar(&self, vessel_id: &str) -> Result<Option<Polar>, DomainError> {
        Ok(self.polars.lock().unwrap().get(vessel_id).cloned())
    }

    fn store_polar(&self, vessel_id: &str, polar: &Polar) -> Result<(), DomainError> {
        self.polars
            .lock()
            .unwrap()
            .insert(vessel_id.to_owned(), polar.clone());
        Ok(())
    }

    fn save_job(&self, job_id: &str, job: &RouteJob) -> Result<(), DomainError> {
        self.jobs
            .lock()
            .unwrap()
            .insert(job_id.to_owned(), job.clone());
        Ok(())
    }

    fn load_job(&self, job_id: &str) -> Result<Option<RouteJob>, DomainError> {
        Ok(self.jobs.lock().unwrap().get(job_id).cloned())
    }
}

struct ConstantWeather(WeatherSample);

impl WeatherSource for ConstantWeather {
    fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
        Ok(self.0)
    }
}

struct UnusedClimatology;

impl ClimatologySource for UnusedClimatology {
    fn sample(&self, _point: GeoPoint, _time_unix: i64) -> Result<WeatherSample, DomainError> {
        Err(DomainError::WeatherOutOfBounds)
    }
}

fn test_state() -> ApiState {
    let weather = ConstantWeather(WeatherSample {
        wind_u: 0.0,
        wind_v: -15.0 * 1_852.0 / 3_600.0,
        gust: 0.0,
        current_u: 0.0,
        current_v: 0.0,
        wave_height: 0.0,
        wave_period: 8.0,
        wave_dir: 0.0,
        is_climatology: false,
    });
    ApiState {
        engine: Arc::new(RouteEngine::new(2).unwrap()),
        weather: Arc::new(weather),
        climatology: Some(Arc::new(UnusedClimatology)),
        spatial: Arc::new(SpatialIndex::new([])),
        store: Arc::new(MemoryStore::default()),
        routing_defaults: RoutingConfig {
            time_step_seconds: 3_600,
            heading_step_rad: 15.0_f64.to_radians(),
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
        },
    }
}

fn route_input(target_lon: f64, overrides: bool) -> CalculateRouteInput {
    CalculateRouteInput {
        vessel_id: "integration-vessel".to_owned(),
        start: CoordinateInput {
            latitude: 0.0,
            longitude: 0.0,
        },
        target: CoordinateInput {
            latitude: 0.0,
            longitude: target_lon,
        },
        departure_time_unix: 0,
        initial_heading_degrees: 90.0,
        time_step_seconds: overrides.then_some(86_400),
        heading_step_degrees: overrides.then_some(15.0),
        max_tws_knots: overrides.then_some(40.0),
        max_wave_height_m: overrides.then_some(5.0),
        max_steps: overrides.then_some(100),
    }
}

async fn http_json_request(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let body = body
        .map(|body| serde_json::to_vec(&body).unwrap())
        .unwrap_or_default();
    let mut stream = TcpStream::connect(address).await.unwrap();
    let headers = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();

    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let headers = std::str::from_utf8(&response[..header_end]).unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let response_body = &response[header_end + 4..];
    let decoded_body = if headers
        .lines()
        .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"))
    {
        decode_chunked(response_body)
    } else {
        response_body.to_vec()
    };
    (status, serde_json::from_slice(&decoded_body).unwrap())
}

fn decode_chunked(mut body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .unwrap();
        let chunk_size =
            usize::from_str_radix(std::str::from_utf8(&body[..line_end]).unwrap(), 16).unwrap();
        body = &body[line_end + 2..];
        if chunk_size == 0 {
            return decoded;
        }
        decoded.extend_from_slice(&body[..chunk_size]);
        body = &body[chunk_size + 2..];
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_and_rest_cover_validation_sync_routes_async_jobs_and_streams() {
    let state = test_state();
    let rest_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rest_address = rest_listener.local_addr().unwrap();
    let rest_state = state.clone();
    let rest_server = tokio::spawn(async move {
        axum::serve(rest_listener, rest_gateway::router(rest_state))
            .await
            .unwrap();
    });

    let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_address = grpc_listener.local_addr().unwrap();
    let grpc_state = state.clone();
    let grpc_server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(grpc_service(grpc_state))
            .serve_with_incoming(TcpListenerStream::new(grpc_listener))
            .await
            .unwrap();
    });

    let mut grpc_client = RouteEngineClient::connect(format!("http://{grpc_address}"))
        .await
        .unwrap();
    let validation = grpc_client
        .validate_polar(PolarValidationRequest {
            vessel_id: "integration-vessel".to_owned(),
            format: "orc_json".to_owned(),
            content: POLAR_JSON.to_owned(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(validation.valid);
    assert_eq!(validation.sail_count, 1);
    assert!((validation.maximum_speed_knots - 6.0).abs() < 1e-9);

    let (status, rest_validation) = http_json_request(
        rest_address,
        "POST",
        "/v1/polars/validate",
        Some(serde_json::json!({
            "vessel_id": "rest-integration-vessel",
            "format": "orc_json",
            "content": POLAR_JSON
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(rest_validation["valid"], true);

    let (status, synchronous) = http_json_request(
        rest_address,
        "POST",
        "/v1/routes/calculate",
        Some(serde_json::to_value(route_input(0.5, false)).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(synchronous["status"], "COMPLETED");
    let final_waypoint = synchronous["waypoints"].as_array().unwrap().last().unwrap();
    assert!(
        (final_waypoint["heading_degrees"].as_f64().unwrap()
            - final_waypoint["course_degrees"].as_f64().unwrap())
        .abs()
            < 1e-9
    );
    assert!(
        (final_waypoint["sog_knots"].as_f64().unwrap() - 6.0).abs() < 1e-9,
        "{final_waypoint}"
    );
    assert!((final_waypoint["longitude"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(
            synchronous["isochrones_geojson_gzip_base64"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    let mut geojson = String::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut geojson)
        .unwrap();
    assert!(geojson.contains("\"type\":\"FeatureCollection\""));

    let (status, accepted) = http_json_request(
        rest_address,
        "POST",
        "/v1/routes/calculate",
        Some(serde_json::to_value(route_input(50.02, true)).unwrap()),
    )
    .await;
    assert_eq!(status, 202);
    let job_id = accepted["job_id"].as_str().unwrap();
    let mut final_job = None;
    for _ in 0..200 {
        let (_, job) =
            http_json_request(rest_address, "GET", &format!("/v1/routes/{job_id}"), None).await;
        if job["status"] != "PROCESSING" {
            final_job = Some(job);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let final_job = final_job.expect("async route job did not finish");
    assert_eq!(final_job["status"], "COMPLETED");
    assert_eq!(final_job["result"]["status"], "COMPLETED");

    let mut stream = grpc_client
        .calculate_route(RouteRequest {
            vessel_id: "integration-vessel".to_owned(),
            start: Some(Coordinate {
                latitude: 0.0,
                longitude: 0.0,
            }),
            target: Some(Coordinate {
                latitude: 0.0,
                longitude: 50.02,
            }),
            departure_time_unix: 0,
            initial_heading_degrees: 90.0,
            time_step_seconds: Some(86_400),
            heading_step_degrees: Some(15.0),
            max_tws_knots: Some(40.0),
            max_wave_height_m: Some(5.0),
            max_steps: Some(100),
        })
        .await
        .unwrap()
        .into_inner();
    let mut isochrone_chunks = 0;
    let mut got_final = false;
    while let Some(message) = stream.message().await.unwrap() {
        if message.event == RouteEvent::IsochroneChunk as i32 {
            isochrone_chunks += 1;
        }
        if message.event == RouteEvent::FinalRoute as i32 {
            assert_eq!(message.status, "COMPLETED");
            got_final = true;
        }
    }
    assert!(isochrone_chunks > 1);
    assert!(got_final);

    rest_server.abort();
    grpc_server.abort();
}

#[test]
#[ignore = "requires Redis and REDIS_TEST_URL"]
fn redis_api_store_persists_polar_and_route_job_state() {
    let redis_url = std::env::var("REDIS_TEST_URL").unwrap();
    let client = redis::Client::open(redis_url).unwrap();
    let store = sailboat_routing_engine::api::RedisApiStore::new(client.clone());
    let vessel_id = format!("api-test-{}", uuid::Uuid::new_v4());
    let job_id = uuid::Uuid::new_v4().to_string();
    let polar = Polar::parse_orc_json(POLAR_JSON).unwrap();
    store.store_polar(&vessel_id, &polar).unwrap();
    assert_eq!(store.load_polar(&vessel_id).unwrap(), Some(polar));

    let job = RouteJob {
        status: sailboat_routing_engine::api::JobStatus::Completed,
        result: None,
        error: None,
    };
    store.save_job(&job_id, &job).unwrap();
    assert_eq!(
        store.load_job(&job_id).unwrap().unwrap().status,
        sailboat_routing_engine::api::JobStatus::Completed
    );

    let mut connection = client.get_connection().unwrap();
    let _: usize = connection
        .del(&[
            Polar::cache_key(&vessel_id).unwrap(),
            format!("route-job:{job_id}"),
        ])
        .unwrap();
}
