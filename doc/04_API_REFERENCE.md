# API Reference

The current service exposes gRPC through Tonic and JSON REST through Axum.
There is no authentication, TLS setup, or rate limiting in the checked-in
server code; deploy behind an appropriately configured trusted network edge.

## Units and conventions

Coordinates are latitude/longitude in degrees (EPSG:4326/WGS84 conventions);
times are UTC Unix seconds. The route core uses metres, m/s, and radians.
Adapters convert headings/courses between degrees and radians, SOG between
knots and m/s, and the request's maximum TWS between knots and m/s.

## Public API-layer Rust types

The public `api` module exposes:

| Type / function | Purpose |
|---|---|
| `ApiState` | Shared route engine, weather/climatology sources, spatial index, storage, and default config; `calculate`, `calculate_with_progress`, `route_distance_m`, and `validate_polar` are the library entry points. |
| `ApiStore` | Storage interface for polar load/store and route-job save/load. |
| `RedisApiStore::new` | Redis implementation of `ApiStore`. |
| `JobStatus`, `RouteJob` | Async job states and serialized status/result/error. |
| `ApiRouteResult`, `ApiWaypoint` | REST-ready result, waypoints, warnings, and encoded isochrones. |
| `CoordinateInput`, `CalculateRouteInput` | Coordinate and routing request DTOs. |
| `PolarValidationInput`, `PolarValidationResult` | Polar validation request and result DTOs. |
| `to_api_result` | Converts a core route to the REST result form. |

These are request adapters and DTOs, not additional network endpoints. The
actual service methods are the two gRPC RPCs and three REST routes below.

## gRPC

The Protobuf package is `routing`; its service is
[`RouteEngine`](../proto/routing.proto).

### `CalculateRoute(RouteRequest) returns (stream RouteResponse)`

Required request fields: `vessel_id`, `start`, `target`,
`departure_time_unix`, and `initial_heading_degrees`. Optional fields override
server defaults: `time_step_seconds`, `heading_step_degrees`, `max_tws_knots`,
`max_wave_height_m`, and `max_steps`.

The server emits:

- `ISOCHRONE_CHUNK` messages as each partial isochrone is produced. These
  include `isochrone_index`, `isochrone_points`, `"PROCESSING"` status, and the
  current `warning_climatology_used` flag.
- One `FINAL_ROUTE` message containing waypoints, final status, and warning
  flag. Each waypoint includes point, Unix timestamp, heading/course degrees,
  SOG knots, fuel remaining, sail index, motor state, and cumulative cost.

If route preparation or execution returns a `DomainError`, it maps to a gRPC
status: invalid coordinates/configuration/polar → `INVALID_ARGUMENT`;
snap, no-safe-route, and weather-bound errors → `FAILED_PRECONDITION`; other
errors → `INTERNAL`. A route that exhausts its search returns
`NO_SAFE_ROUTE_FOUND` as a final response status rather than necessarily
terminating the RPC with a status error.

### `ValidatePolar(PolarValidationRequest) returns (PolarValidationResponse)`

`format` must be `orc_json`, `csv_normalized`, or `csv_matrix`. `content` holds
the complete source representation. A valid parsed polar is persisted to
Redis under `vpp:{vessel_id}`. The response includes `valid`,
`maximum_speed_knots`, and `sail_count`.

## REST

The REST router exposes:

### `POST /v1/routes/calculate`

Request JSON:

```json
{
  "vessel_id": "boat-42",
  "start": { "latitude": 41.49, "longitude": -71.31 },
  "target": { "latitude": 32.3, "longitude": -64.78 },
  "departure_time_unix": 1760000000,
  "initial_heading_degrees": 90,
  "time_step_seconds": 3600,
  "heading_step_degrees": 10,
  "max_tws_knots": 40,
  "max_wave_height_m": 5,
  "max_steps": 100
}
```

`initial_heading_degrees` defaults to `0`; the optional routing overrides may
be omitted to use server defaults. The geodesic start-to-target distance
selects the response mode:

- Below 200 NM: HTTP 200 with a route result.
- At or above 200 NM: HTTP 202 with a background Redis job identifier.

Example synchronous result shape:

The values below are illustrative and abbreviated to two waypoints.

```json
{
  "status": "COMPLETED",
  "waypoints": [
    {
      "latitude": 41.49,
      "longitude": -71.31,
      "time_unix": 1760000000,
      "heading_degrees": 90,
      "course_degrees": 90,
      "sog_knots": 5.2,
      "fuel_remaining_l": 0,
      "sail_index": 0,
      "is_motoring": false,
      "cumulative_cost_s": 0
    },
    {
      "latitude": 32.3,
      "longitude": -64.78,
      "time_unix": 1760100000,
      "heading_degrees": 210,
      "course_degrees": 210,
      "sog_knots": 5.2,
      "fuel_remaining_l": 0,
      "sail_index": 0,
      "is_motoring": false,
      "cumulative_cost_s": 100000
    }
  ],
  "warnings": [],
  "isochrones_geojson_gzip_base64": "<gzip-compressed GeoJSON FeatureCollection, Base64>"
}
```

An asynchronous acceptance response is:

```json
{ "job_id": "generated-uuid", "status": "PROCESSING" }
```

### `GET /v1/routes/{job_id}`

Returns HTTP 200 with a `RouteJob` object when found. `status` is
`PROCESSING`, `COMPLETED`, or `FAILED`. A completed job has `result`; a failed
job may have an error and, for a no-safe-route outcome, may also include the
route result. Jobs expire after 24 hours. An unknown job returns HTTP 404:

```json
{ "error": "route job not found" }
```

### `POST /v1/polars/validate`

Request:

```json
{
  "vessel_id": "boat-42",
  "format": "orc_json",
  "content": "{\"metadata\":{\"boatName\":\"Test\",\"unitTWS\":\"knots\",\"unitBS\":\"knots\"},\"tws_grid\":[5,10],\"twa_grid\":[0,90,180],\"configurations\":[{\"sail_id\":\"main\",\"matrix\":[[0,0],[2,4],[1,2]]}]}"
}
```

On success, HTTP 200 returns:

```json
{ "valid": true, "maximum_speed_knots": 4, "sail_count": 1 }
```

Supported `format` values are `orc_json`, `csv_normalized`, and `csv_matrix`.
Successful validation stores the polar in Redis; validation is not a
side-effect-free dry run.

### REST errors and routing outcomes

REST errors use `{ "error": "..." }`. Invalid coordinate/configuration/polar
inputs map to HTTP 400; snap, weather, and no-safe-route domain errors map to
HTTP 422; remaining domain/storage errors map to HTTP 500. However, synchronous
search exhaustion is a `RouteResult` status, not a `DomainError`, and is
serialized as HTTP 200 with `"status":"NO_SAFE_ROUTE_FOUND"`. For long jobs the
same outcome is marked `FAILED` with `ERR_NO_SAFE_ROUTE_FOUND`.

`ERR_INVALID_START_COORD` is the domain message for failure to find an
obstacle-free snap point; the same snap function is also used on the target.
`WARNING_CLIMATOLOGY_USED` appears in the REST `warnings` array when a caller
supplies and uses climatology. `ERR_NO_SAFE_ROUTE_FOUND` is emitted for the
asynchronous failed-job error; the synchronous result uses the status string
above. Other domain errors have descriptive messages but not all have stable
`ERR_*` identifiers.

## NMEA 2000 library interface

NMEA monitoring is implemented in
[`src/api/nmea_reroute.rs`](../src/api/nmea_reroute.rs), not as an HTTP or gRPC
endpoint. `decode_pgn` accepts exactly eight-byte payloads for:

| PGN | Decoded fields |
|---|---|
| `129026` | COG/SOG rapid update; only true-north course reference is accepted. COG uses 0.0001 rad units; SOG uses 0.01 m/s units. |
| `130306` | Wind speed/direction; speed uses 0.01 m/s units and angle uses 0.0001 rad units. Reference bits are returned. |
| `127250` | Heading; heading uses 0.0001 rad units. Reference bits are returned. |

`NmeaRerouteMonitor` uses COG/SOG to dead-reckon from the last position,
computes minimum cross-track distance to route segments, and emits one
`NmeaRerouteEvent` when the configured XTE threshold is crossed. It rearms
after the estimated position returns within the threshold. The event can
produce a new `CalculateRouteInput` preserving the route target. Wind and
heading PGNs are decoded but do not currently drive dead reckoning. There is
no live NMEA socket/serial ingress, persistence, or automatic call from the
event into `ApiState::calculate`.

## Domain error catalog

The authoritative variants are in [`src/error.rs`](../src/error.rs):

| Variant / message prefix | Meaning |
|---|---|
| `InvalidStartCoord` / `ERR_INVALID_START_COORD` | No water point found within snap radius |
| `NoSafeRouteFound` / `ERR_NO_SAFE_ROUTE_FOUND` | All candidate branches pruned (also used by bounded A* failure) |
| `InvalidCoordinate` | Latitude/longitude outside valid range |
| `InvalidDistance`, `InvalidBearing`, `GeodesyDidNotConverge` | Invalid or non-convergent geodesic operation |
| `InvalidSnapRadius` | Negative/non-finite snap radius |
| `Postgis`, `InvalidPostgisGeometry` | Database query or geometry conversion failure |
| `InvalidZarr`, `WeatherOutOfBounds`, `MissingWeatherData` | Zarr layout/metadata or weather query failure |
| `InvalidPolar`, `PolarCache` | Polar parse/validation or Redis cache failure |
| `InvalidKinematics`, `InvalidWeatherBlend`, `InvalidCostInput` | Invalid physical, blend, or edge-cost input |
| `InvalidEnsembleInput` | Invalid departure/ensemble/CVaR input |
| `InvalidNmeaFrame`, `UnsupportedNmeaPgn` | Malformed frame or unsupported PGN |
| `InvalidRouteConfig` | Invalid route search configuration |
| `RouteJobStore` | Async route-job serialization or Redis failure |
