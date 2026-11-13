# Data and Storage

## Weather ingestion and Zarr layout

[`ingestion/weather_etl.py`](../ingestion/weather_etl.py) reads GRIB/GRIB2
through `cfgrib` or NetCDF through xarray, selects variables on the configured
latitude/longitude/time dimensions, normalizes dimension names to
`[lat, lon, time]`, and writes Zarr v2. It does not download forecasts or
schedule ingestion jobs.

The default chunk sizes are `lat=100`, `lon=100`, `time=240`, bounded by the
source dimension lengths. Arrays use C order and no compressor or filters so
the Rust reader can mmap chunk files. The time coordinate is converted to
UTC Unix seconds (`int64`). Source array units are retained as Zarr metadata.
The CLI can write a local store or use fsspec for an `s3://` destination.

At service startup, [`WeatherGrid4D::open_zarr`](../src/weather/interpolator.rs)
expects coordinate arrays named `lat`, `lon`, `time` and eight field arrays:

| Default array name | Meaning | Accepted units / normalized unit |
|---|---|---|
| `wind_u`, `wind_v`, `gust` | Wind components and gust | `m/s` or knots → m/s |
| `current_u`, `current_v` | Current components | `m/s` or knots → m/s |
| `wave_height` | Significant/selected wave height field | metres or centimetres → metres |
| `wave_period` | Wave period | seconds or minutes → seconds |
| `wave_dir` | Wave direction | radians or degrees → radians |

The eight names can be overridden individually by `WEATHER_*` variables. Each
field must have `[lat, lon, time]` dimensions and shape matching the coordinate
axes. Only Zarr v2, C-order, uncompressed arrays with supported numeric dtypes
are accepted. The Rust reader uses read-only mmap mappings and accesses values
from mapped chunks; it does not implement remote S3 reads or Zarr codecs.

The term “4D weather” describes a field over two spatial axes, time, and
multiple meteorological variables. The current interpolator brackets three
axes (`lat`, `lon`, `time`) per scalar field, not a fourth independent
coordinate such as depth or ensemble-member index.

## PostGIS schema

[`migrations/001_initial_spatial.sql`](../migrations/001_initial_spatial.sql)
creates PostGIS tables:

| Table | Columns beyond primary key | Indexes / current use |
|---|---|---|
| `coastlines` | `source`, `geom MultiPolygon(4326)`, `active_from`, nullable `active_to` | GiST on `geom`; active-period B-tree. Imported by `spatial_etl.py`; loaded by service. |
| `bathymetry` | non-negative `depth_m`, `geom MultiPolygon(4326)`, validity interval | GiST and active-period B-tree. Schema only; no current importer or route query. |
| `no_go_zones` | `name`, `geom MultiPolygon(4326)`, required `active_from`, nullable `active_to` | GiST and active-period B-tree. Service loads active geometry. |

The service's `SpatialIndex::load_postgis` selects active coastlines and no-go
zones using the startup Unix timestamp, converts geometries from GeoJSON to
polygons, and bulk-loads them into an in-memory R-tree. It does not keep a live
database cursor or refresh the index while running. The migration has no
spatial query/view that composes bathymetry into route feasibility.

The coastline ETL requires a source CRS. It repairs input geometry, projects
to a local azimuthal-equidistant CRS, buffers by 50 m, simplifies with
topology preservation (100 m default), transforms to EPSG:4326, and inserts
the features. Features are read in batches of 1,000, but the current insert
helper materializes the transformed WKB rows before `executemany`.

## Redis keys and lifecycle

[`RedisApiStore`](../src/api/mod.rs) and the polar cache use two key families:

| Key | Payload | Expiration |
|---|---|---|
| `vpp:{vessel_id}` | JSON serialization of validated `Polar` axes, sails, speed matrices, and VMG data | No TTL is set by the current polar cache code |
| `route-job:{job_id}` | JSON `RouteJob` containing status and optional result/error | 86,400 seconds (24 hours) |

The route result stores waypoints plus partial isochrones encoded as a
GeoJSON `FeatureCollection`, gzip-compressed and Base64-encoded. This is an API
serialization choice, not a Redis-side GeoJSON index.

Redis persistence, replication, eviction policy, and high availability are
deployment concerns not configured in this repository.

## Memory and sharing

The Zarr arrays are opened as memory maps by [`MmapZarrArray`](../src/weather/zarr_mmap.rs).
This avoids eagerly decoding/copying the entire dataset into Rust-owned
buffers; individual page residency is controlled by the operating system.
It is not proof of the SRS's 6–10 GB production-model footprint or of a
multi-process shared-memory deployment. The Criterion benchmark uses a tiny
synthetic dataset and its process RSS high-water mark includes the benchmark
process, not an isolated production model.

## Data pipeline gaps

- No GRIB forecast download/orchestration, freshness monitor, or S3-to-local
  staging is included.
- No bathymetry ingestion or depth-aware collision check is wired in.
- The server uses a local filesystem path for Zarr, despite the weather ETL's
  S3 output capability.
- The default server configuration does not provide a climatology store.
