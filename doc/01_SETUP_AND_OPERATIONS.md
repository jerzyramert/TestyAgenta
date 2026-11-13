# Setup and Operations

## Requirements

| Component | Repository requirement / validation |
|---|---|
| Rust | Edition 2021; `rust-version = "1.80"` in [`Cargo.toml`](../Cargo.toml). The current audit ran with stable Rust 1.100; Rust 1.80 itself was not tested. |
| Python | Python `>=3.11` in [`ingestion/pyproject.toml`](../ingestion/pyproject.toml). This audit used Python 3.12.3. |
| Protobuf compiler | The build uses `protoc-bin-vendored`; a separate system `protoc` is not required by the checked-in Cargo build. |
| PostgreSQL / PostGIS | Required by the service's spatial startup loader. Run the provided migration with PostGIS available. No server version is pinned in the repository. |
| Redis | Required by the service for polar lookup and asynchronous route-job state. `REDIS_URL` must be reachable at startup. |
| Weather storage | A local Zarr v2 store is required by the Rust process. The Python ETL can write local paths or `s3://` output, but the Rust reader accepts a local filesystem path, not an S3 URL. |
| GRIB2 support | `cfgrib` and `eccodes` are Python dependencies. Native ecCodes availability may depend on the platform/package environment. |
| Spatial input | GeoPandas/pyogrio and the relevant GDAL drivers are used to read spatial source files, including S-57 sources where supported by the installed GDAL build. |

There is no checked-in Docker Compose file, deployment manifest, or `.env`
loader. The Rust service reads process environment variables directly. Use
your shell, container runtime, or process manager to provide them.

## Database initialization

Create a database with PostGIS installed, then apply the schema:

```sh
psql "$POSTGIS_DATABASE_URL" -v ON_ERROR_STOP=1 \
  -f migrations/001_initial_spatial.sql
```

The migration creates `coastlines`, `bathymetry`, and `no_go_zones` with
temporal validity columns and GiST geometry indexes. The service currently
loads active coastline and no-go geometries into memory at startup; it does not
load bathymetry.

## Python environment and ETL

From the repository root, create an environment and install the project
dependencies:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install -e './ingestion[dev]'
```

Convert a GRIB2 or NetCDF dataset into uncompressed Zarr v2:

```sh
python -m ingestion.weather_etl \
  --input ./data/forecast.grib2 \
  --output ./data/forecast.zarr \
  --variable wind_u --variable wind_v --variable gust \
  --variable current_u --variable current_v \
  --variable wave_height --variable wave_period --variable wave_dir
```

`--engine` can override xarray's inferred engine. `--latitude-dim`,
`--longitude-dim`, and `--time-dim` override source dimension names. Variables
must share exactly those three dimensions. Output uses canonical `lat`, `lon`,
and `time` dimensions and stores time as UTC Unix seconds. Default chunk sizes
are up to 100 latitude values, 100 longitude values, and 240 time values; each
is reduced to the corresponding dimension size for smaller datasets. Output
can also be an `s3://` URI when the environment has suitable fsspec/S3
credentials and configuration.

The service's default variable names are `wind_u`, `wind_v`, `gust`,
`current_u`, `current_v`, `wave_height`, `wave_period`, and `wave_dir`.
If source array names differ, set the corresponding `WEATHER_*` environment
variables so they match the Zarr array directory names.

Load buffered, simplified coastline features into PostGIS:

```sh
DATABASE_URL="$POSTGIS_DATABASE_URL" \
  python -m ingestion.spatial_etl \
    --source ./data/coastlines.gpkg \
    --layer coastlines \
    --simplify-tolerance-m 100
```

`--dsn` overrides `DATABASE_URL`. The importer requires a source CRS, transforms
features to EPSG:4326, repairs polygon geometry, applies a 50 m coastline
buffer in a local azimuthal-equidistant projection, simplifies with topology
preserved (100 m by default), then inserts into `coastlines`. The script does
not import bathymetry or no-go zones.

## Service configuration

The following variables are required:

| Variable | Meaning / units |
|---|---|
| `REDIS_URL` | Redis connection URL |
| `WEATHER_ZARR_PATH` | Local Zarr v2 directory |
| `POSTGIS_DATABASE_URL` | PostgreSQL connection URL |
| `ROUTING_TIME_STEP_SECONDS` | Propagation interval, seconds |
| `ROUTING_HEADING_STEP_DEGREES` | Heading sample interval, degrees; converted to radians |
| `ROUTING_MAX_TWS_KNOTS` | Maximum true-wind speed, knots; converted to m/s |
| `ROUTING_MAX_WAVE_HEIGHT_M` | Wave-height pruning limit, metres |
| `ROUTING_MAX_STEPS` | Maximum propagation steps |
| `ROUTING_LEEWAY_COEFF` | Non-negative leeway coefficient used by the implemented formula |
| `ROUTING_HEEL_DEGREES` | Heel input, degrees; converted to radians |
| `ROUTING_INITIAL_FUEL_L` | Initial fuel, litres |
| `ROUTING_TACK_PENALTY_S` | Tack penalty, seconds |
| `ROUTING_GYBE_PENALTY_S` | Gybe penalty, seconds |
| `ROUTING_SAIL_CHANGE_PENALTY_S` | Sail-change penalty, seconds |
| `ROUTING_FUEL_WEIGHT_S_PER_L` | Fuel cost weight, seconds per litre |
| `ROUTING_WAVE_WEIGHT_S_PER_M` | Excess-wave cost weight, seconds per metre |
| `ROUTING_WAVE_COMFORT_HEIGHT_M` | Wave-height threshold below which no wave cost is added, metres |

Motor-sailing is disabled when all three optional variables are absent. They
must be supplied together:

| Variable | Meaning / units |
|---|---|
| `ROUTING_MIN_MOTORING_SOG_KNOTS` | SOG threshold below which motoring may start |
| `ROUTING_MOTORING_SOG_KNOTS` | Fixed motoring SOG |
| `ROUTING_MOTOR_FUEL_RATE_LPH` | Fuel consumption rate, litres per hour |

The two knot values are converted to m/s at startup. Partial motor
configuration is a startup error.

Weather array names can be overridden with `WEATHER_WIND_U`, `WEATHER_WIND_V`,
`WEATHER_GUST`, `WEATHER_CURRENT_U`, `WEATHER_CURRENT_V`,
`WEATHER_WAVE_HEIGHT`, `WEATHER_WAVE_PERIOD`, and `WEATHER_WAVE_DIRECTION`.
The listener defaults are `REST_ADDR=0.0.0.0:8080` and
`GRPC_ADDR=0.0.0.0:50051`.

The server loads the active spatial features and memory-maps the configured
weather arrays once at startup. Restart the service after replacing its loaded
dataset or updating the active spatial snapshot. No explicit `/dev/shm` or
shared-memory configuration is implemented: mmap-backed files use the host OS
page cache. Shared pages across processes depend on the OS and identical file
mappings; the service does not coordinate a shared-memory object itself.

## Start the server

After exporting the required variables above:

```sh
cargo run --release
```

The process starts the REST and gRPC listeners together. If either listener
fails, startup returns an error. Redis, PostGIS, and the Zarr store must be
reachable/valid before serving requests.

## Tests and benchmarks

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test --workspace
cargo test --test golden_cases

cd ingestion
ruff check .
mypy .
pytest
```

`POSTGIS_TEST_DSN` enables the PostGIS-backed spatial integration test;
`REDIS_TEST_URL` enables Redis cache/API integration tests. Without those
variables, the associated Rust tests are ignored and the PostGIS Python test is
skipped.

Criterion benchmarks run without external services:

```sh
cargo bench --bench routing_bench -- \
  --sample-size 10 --warm-up-time 1 --measurement-time 1
```

Criterion reports are written under `target/criterion/`. Benchmark values are
fixture- and machine-dependent; see
[Testing and Benchmarks](./05_TESTING_AND_BENCHMARKS.md) before interpreting
them as SLA evidence.
