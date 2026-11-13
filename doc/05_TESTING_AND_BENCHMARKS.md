# Testing and Benchmarks

## Verification commands

Rust checks run from the repository root:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test --workspace
cargo test --test golden_cases
```

Python ETL checks run from `ingestion/`:

```sh
ruff check .
mypy .
pytest
```

## Golden routing cases

The five required named cases are in
[`tests/golden_cases/`](../tests/golden_cases/), assembled by
[`tests/golden_cases.rs`](../tests/golden_cases.rs). Common fixtures are
deterministic and use a 15-knot synthetic wind unless a case overrides it.

| Case | Inputs and verified behavior | Assertions / tolerances |
|---|---|---|
| `test_upwind_tacking` | Northbound route into northerly wind, synthetic polar favoring a 42° beat | Reaches within 0.5 NM; more than two legs near 42°/318°; course changes sides of the wind axis; no positive-speed course outside the expected beat angles ±2°. |
| `test_current_drift` | 5 kt STW north with 2 kt east current | SOG equals `sqrt(5²+2²)` kt and COG equals `atan(2/5)` radians, each within 0.001 in SI/radian comparisons. |
| `test_island_avoidance` | Route around a synthetic island of roughly 10 NM radius with a 50 m margin | Route completes at the target and no adjacent waypoint segment intersects the obstacle. |
| `test_antimeridian` | Route from 179.5°E to 179.5°W | Route completes, crosses the longitude seam, and summed geodesic leg distance is 60 NM ±0.5 NM. |
| `test_edge_cases` | Start inside a 1 NM-radius obstacle; a second obstacle extending 6 NM; 60 kt storm against a 40 kt limit; forecast horizon plus supplied climatology; calm wind with motor configuration | First route snaps and completes; distant snap fails with `InvalidStartCoord`; storm returns `NoSafeRouteFound` plus partial isochrones; climatology completes with warning; motoring is used in calm weather and fuel does not increase. |

The golden tests assert the implemented domain results. A synchronous
no-route result is `RouteStatus::NoSafeRouteFound`, not
`Err(DomainError::NoSafeRouteFound)`.

## Other test coverage

Rust unit tests cover polar formats/interpolation/cache payload validation,
wind/current/leeway/motor calculations, costs, forecast fallback, collision
queries, antimeridian geodesy, mmap weather interpolation and no-heap-allocation
queries, H3 search, blending, ensemble CVaR, departure optimization, and NMEA
decoding/rearming. The REST/gRPC integration test exercises validation,
synchronous routes, asynchronous jobs, and streaming.

The current full run completed with:

- Rust: 55 library tests passed, 1 Redis-dependent library test ignored; 1
  binary test passed; 1 API integration test passed, 1 Redis-dependent
  integration test ignored; all 5 golden tests passed; 0 doc tests.
- Python: Ruff passed; mypy reported no issues in 5 source files; pytest
  reported 8 passed and 1 PostGIS-dependent test skipped.
- `cargo fmt --check` and `cargo clippy -- -D warnings` passed.

The current environment did not provide `REDIS_TEST_URL` or
`POSTGIS_TEST_DSN`, so those live-service cases were not part of this final
run. Earlier entries in [`PROGRESS.md`](../PROGRESS.md) record successful
service-backed runs, but this result should not be read as a fresh live
Redis/PostGIS verification.

## Criterion benchmark evidence

[`benches/routing_bench.rs`](../benches/routing_bench.rs) measures:

- 100,000 weather interpolation queries on a tiny synthetic mmap Zarr store;
- 100,000 repeated segment collision checks against 1,001 synthetic polygons;
- a synthetic 120 NM route and a synthetic >3,000 NM route using constant
  weather, no route obstacles, and a synthetic polar;
- process `ru_maxrss` after a transatlantic route execution.

Representative longer-window run on the current macOS environment
(`--sample-size 15 --warm-up-time 3 --measurement-time 4`):

| Benchmark | Estimate / interval |
|---|---:|
| 100,000 weather queries | 105.36 ms [100.39, 114.79] ms |
| 100,000 collision checks | 134.01 ms [38.34, 282.95] ms; highly variable |
| 120 NM route | 39.279 ms [30.841, 50.083] ms; variable |
| >3,000 NM route | 271.60 ms [265.52, 279.31] ms |
| Process peak RSS after route | 24,625,152 bytes |

A shorter reduced run measured 104.84 ms for weather interpolation, 14.239 ms
for collision checks, 10.121 ms for the 120 NM route, 274.84 ms for the long
route, and 32,194,560 bytes process peak RSS. The large spread in collision,
short-route, and RSS results indicates substantial environmental/measurement
variation. Treat the long-route timing as a synthetic smoke measurement, not a
production SLA result. The benchmark process's RSS is a high-water mark and is
not an isolated allocation measurement for one route.

Criterion's initial reduced run also reported increases against its saved local
baseline of approximately 67% for weather interpolation, 93% for collision
checks, 67% for the 120 NM route, and 71% for the long route. A later longer
run did not find a statistically significant change for weather interpolation
or the long route relative to the preceding sample, while collision and short
route measurements had large outliers. Because the benchmark environment and
the old stored baseline are not pinned or independently reproduced, this is a
performance regression signal to investigate, not proof that the recent code
change caused a regression.

The WBS's microbenchmark target of less than 5 ms for 100,000 weather queries
is not met by the measured approximately 105 ms. This target is distinct from
the SRS request-latency SLA. The benchmark does not justify a production
latency claim: it uses synthetic fixtures, does not measure representative
concurrent load, does not calculate a service P95, and does not time the first
gRPC chunk.

## SRS SLA status

The SRS specifies local-route P95 below 500 ms, >3,000 NM route P95 below 5 s,
and first gRPC chunk below 500 ms. The synthetic medians observed here are
below the first two numerical limits, but there is no representative load test,
P95 distribution, real forecast dataset, obstacle dataset, or first-chunk
measurement. Therefore those SLAs remain **unverified**, not passed.

The SRS also describes an approximately 6–10 GB production weather model and
shared-memory reuse. The tiny benchmark fixture and process RSS values above
do not verify that production memory target or cross-process sharing.

## Known gaps in verification

- No pinned real-world GRIB/NetCDF/ORC reference-data suite or independent
  cross-validation against a reference routing product is in this repository.
- A PostGIS-backed Python test and two Redis-backed Rust tests require external
  services and were skipped/ignored in the final run.
- Rust 1.80 is declared as the MSRV but was not used for this audit.
- No P95/concurrency test, production-scale data test, gRPC first-chunk timing,
  or dependable peak-RSS benchmark is currently present.
- The 5 ms per 100,000 weather-query WBS target is unmet; resolve the target
  definition or optimize and remeasure before treating it as an acceptance
  gate.
