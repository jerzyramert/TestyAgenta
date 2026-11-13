# Verification Audit

## Executive summary

The declared Rust/Python checks and all five required golden tests pass in the
current environment. The audit also fixed a motor-sailing dead end in calm
weather, exposed motor configuration through startup variables, added the
missing golden-case files, and made the prescribed Python test command resolve
the ingestion package from its project directory.

This is not a full requirements sign-off. The SRS defines only FR-001 through
FR-010, despite the audit prompt requesting FR-011/FR-012. Several Pro and
real-time components exist as standalone library modules but are not part of
the ordinary route/API flow. Wave resistance is explicitly deferred; dynamic
UKC remains blocked by Q-004. Synthetic benchmarks do not verify production
P95 or memory SLAs, and the WBS's `<5 ms / 100,000 weather queries` target is
not met.

## Scope and source-document mapping

The audit prompt names `AGENTS.md`, `weather_routing_engine_sdd.md`, and
`execution_plan.md`. Those exact paths are absent. The repository equivalents
read for this audit are:

| Prompt name | Repository file |
|---|---|
| `AGENTS.md` | [`AGENT.MD`](../AGENT.MD) |
| `weather_routing_engine_sdd.md` | [`software_design_document_srs.md`](../software_design_document_srs.md) |
| `execution_plan.md` | [`plan_implementacyjny_i_wbs.md`](../plan_implementacyjny_i_wbs.md) |
| `PROGRESS.md` | [`PROGRESS.md`](../PROGRESS.md) |
| `ANALYST_QUESTIONS.md` | [`ANALYST_QUESTIONS.md`](../ANALYST_QUESTIONS.md) |

The audit also inspected [`BYPASS_FUTURES.md`](../BYPASS_FUTURES.md),
[`polar.md`](../polar.md), the Rust/Python implementation, migration,
Protobuf contract, tests, and Criterion benchmark.

## Verification evidence

Commands completed successfully:

```text
cargo fmt --check
cargo clippy -- -D warnings
cargo test --workspace
cd ingestion && ruff check . && mypy . && pytest
cargo test --test golden_cases
```

Results: 55 Rust library tests passed and 1 Redis test was ignored; 1 binary
test passed; 1 API integration test passed and 1 Redis test was ignored; all 5
golden tests passed. Python Ruff and mypy passed; pytest reported 8 passed and
1 PostGIS-dependent skip. The final environment did not set `REDIS_TEST_URL`
or `POSTGIS_TEST_DSN`.

Static inspection found no production `unwrap()`, `expect()`, `todo!()`,
`unimplemented!()`, or `// TODO` use under `src/`. Textual `unwrap()`/
`expect()` matches are in `#[cfg(test)]` code. No `// ASSUMPTION [Q-XXX]`
comments were found under `src/`.

Core calculations generally use SI-suffixed values (`*_m`, `*_mps`, `*_rad`)
and UTC Unix seconds; the API converts degrees/knots at its boundaries. Unit
wrappers exist in `types.rs`, although many data structures still use `f64`
with unit-bearing field names rather than enforcing wrappers in the type
system.

## Functional requirements traceability

Status definitions: **PASS** means the implemented behavior and relevant
automated verification are present for the requirement's current scope;
**PARTIAL** means a component exists but acceptance behavior/integration or
production evidence is missing; **FAIL/DEFERRED** means the requested
capability is absent or not accepted for the current MVP.

| Requirement | Implementation evidence | Verification evidence | Status |
|---|---|---|---|
| FR-001 — GRIB2/NetCDF to Zarr | [`ingestion/weather_etl.py`](../ingestion/weather_etl.py), [`src/weather/zarr_mmap.rs`](../src/weather/zarr_mmap.rs) | `tests/test_weather_etl.py`; mmap reader tests | **PARTIAL** — conversion is implemented, but source acquisition/orchestration and production-feed validation are absent. |
| FR-002 — space-time interpolation and seam handling | [`src/weather/interpolator.rs`](../src/weather/interpolator.rs), [`src/spatial/geodesy.rs`](../src/spatial/geodesy.rs) | Weather interpolation, circular direction, and antimeridian tests; `test_antimeridian` | **PASS** for implemented grid/interpolation behavior. |
| FR-003 — regional blending and ensemble routing | [`src/weather/blender.rs`](../src/weather/blender.rs), [`src/core/ensemble.rs`](../src/core/ensemble.rs) | Blend and CVaR/departure unit tests | **PARTIAL** — standalone functions exist; no weather-source composition, EPS ingestion, or API exposure. |
| FR-004 — VPP STW evaluation | [`src/physics/polar.rs`](../src/physics/polar.rs) | Parser, interpolation, invalid-cell, VMG, and crossover tests | **PASS** for supported formats and implemented interpolation; no independent vessel-data accuracy validation. |
| FR-005 — motor-sailing and penalties | [`src/physics/kinematics.rs`](../src/physics/kinematics.rs), [`src/core/cost.rs`](../src/core/cost.rs), [`src/core/isochrone.rs`](../src/core/isochrone.rs) | Fuel/threshold/cost tests; calm motor case in `test_edge_cases` | **PASS** for implemented deterministic rules. |
| FR-006 — ISO 15016/Kwon wave resistance | No resistance module exists under `src/physics/`; see [`BYPASS_FUTURES.md`](../BYPASS_FUTURES.md) | Q-003; no physics test | **DEFERRED** — explicitly excluded from MVP. |
| FR-007 — modified isochrone and bearing buckets | [`src/core/isochrone.rs`](../src/core/isochrone.rs) | Search/pruning tests and upwind, island, seam, and edge golden cases | **PASS** for current synthetic routing acceptance; not production-load validated. |
| FR-008 — no-sail tapering | [`src/physics/polar.rs`](../src/physics/polar.rs) | Polar taper and upwind tacking tests | **PASS** for the implemented polar taper and synthetic beating behavior. |
| FR-009 — H3/A* corridor | [`src/core/astar_corridor.rs`](../src/core/astar_corridor.rs) | Determinism, narrow-passage, and bounded-search tests | **PARTIAL** — corridor builder works independently but is not used to constrain route-engine propagation. |
| FR-010 — NMEA dynamic re-routing | [`src/api/nmea_reroute.rs`](../src/api/nmea_reroute.rs) | PGN decoding, monotonic-time, XTE/rearm tests | **PARTIAL** — monitor emits an event and updated request; no live bus ingress or automatic service route execution. |
| FR-011 | No definition found in the SRS or current source requirements | None | **UNSPECIFIED** — no requirement can be traced or claimed. |
| FR-012 | No definition found in the SRS or current source requirements | None | **UNSPECIFIED** — no requirement can be traced or claimed. |

## Implementation-step traceability

The step states below follow [`PROGRESS.md`](../PROGRESS.md), checked against
the implementation and tests. Completion of a standalone module does not imply
production-path integration.

| Step | Evidence / state | Status |
|---|---|---|
| STEP-01 | Rust workspace, SI-related primitives, and checks | **PASS** |
| STEP-02 | WGS84 direct/inverse geodesy; seam and precision tests | **PASS** |
| STEP-03 | PostGIS schema and buffered/simplified coastline ETL | **PASS**, live PostGIS test skipped in this final run |
| STEP-04 | R-tree, antimeridian-safe collision, snap-to-water | **PASS** |
| STEP-05 | Polar parsers/interpolator and Redis cache | **PASS**, live Redis test skipped in this final run |
| STEP-06 | Current/leeway/motor kinematics and costs | **PASS** |
| STEP-07 | GRIB/NetCDF ingestion to Zarr v2 | **PASS** for pipeline code; real-feed operations not verified |
| STEP-08 | Read-only mmap and space-time interpolation | **PASS**, with the WBS query-throughput target unmet |
| STEP-09 | Edge costs and maneuver penalties | **PASS** |
| STEP-10 | Isochrone routing and pruning | **PASS** for covered synthetic cases |
| STEP-11 | gRPC/REST service and async Redis jobs | **PASS** in non-Redis API integration; Redis persistence skipped here |
| STEP-12 | Wave resistance/UKC | **BLOCKED** by Q-004; wave resistance separately excluded by Q-003 |
| STEP-13 | H3 corridor and regional blending modules | **PARTIAL integration** — module tests pass, normal route path does not compose them |
| STEP-14 | Departure optimization, ensemble, NMEA monitor | **PARTIAL integration** — libraries/tests exist; no service/API wiring for these workflows |
| STEP-15 | Criterion benchmarks and RSS report | **PARTIAL verification** — benchmark executes, but results are noisy/synthetic and do not establish production SLA |

## Non-functional requirements and edge behavior

| Requirement / behavior | Evidence and result | Status |
|---|---|---|
| Local route P95 <500 ms | Synthetic 120 NM route was measured at 39.279 ms in the longer run; no real-data load/P95 test | **UNVERIFIED** |
| >3,000 NM route P95 <5 s | Synthetic long route estimate 271.60 ms; no service-level or P95 test | **UNVERIFIED** |
| First streamed gRPC chunk <500 ms | Streaming integration exists, but no first-chunk latency assertion | **UNVERIFIED** |
| WBS target <5 ms / 100,000 weather queries | Longer run: 105.36 ms estimate [100.39, 114.79] ms | **FAIL** — approximately 21× target |
| R-tree <1 ms / 1,000 edges | Current repeated-segment microbenchmark was highly variable and not representative of 1,000 distinct route edges | **UNVERIFIED** |
| 6–10 GB production weather memory and shared memory | Only tiny synthetic mmap fixture measured; no production dataset or multi-process verification | **UNVERIFIED** |
| Forecast exhaustion | Router can use `ClimatologySource`; production `main.rs` sets it to `None` | **PARTIAL** |
| Total storm blockade | Route returns `RouteStatus::NoSafeRouteFound` and retains partial isochrones | **PASS**; synchronous API does not return a `DomainError` as the SRS wording suggests |
| Start/target on land | Both are passed through snap-to-water within 2 NM; failure uses `InvalidStartCoord` | **PARTIAL** — uses currently loaded obstacles, not bathymetry or a navigability model |

Benchmark details and repeat variability are recorded in
[Testing and Benchmarks](./05_TESTING_AND_BENCHMARKS.md).

## Gaps, discrepancies, and technical debt

1. **Prompt/spec filenames and FR numbering:** The prompt's three named source
   files do not exist at those paths; repository equivalents were used. The SRS
   stops at FR-010. FR-011/012 remain undefined and were not invented.
2. **Weather query throughput:** The WBS's `<5 ms / 100,000 queries` target is
   not achieved. The current implementation does pass the functional
   interpolation/no-allocation tests. Treat the throughput threshold as an
   unresolved performance gate; do not describe it as passed.
3. **Benchmark regression signal:** The first current Criterion run reported
   increases of about 67% for weather, 93% for collision checks, 67% for the
   120 NM route, and 71% for the long route against the saved baseline. The
   longer repeat found no significant change for weather or long-route timing,
   while collision and short-route measurements remained noisy. The available
   data flags a regression risk but does not establish cause.
4. **SLA evidence:** Criterion fixtures are synthetic and single-process.
   There is no P95/concurrency benchmark, production weather/spatial data run,
   or gRPC first-chunk timing. The current results cannot certify the SRS SLA.
5. **Incomplete feature composition:** A* corridors, regional blending,
   ensemble evaluation, departure optimization, and NMEA monitoring have
   tests, but are not composed into the normal route calculation/API.
6. **No production climatology:** The core supports fallback, but the standard
   server constructs `ApiState` with `climatology: None`.
7. **No bathymetry-based safety check:** `bathymetry` exists in SQL but is not
   imported by the provided spatial ETL or loaded into `SpatialIndex`.
8. **Wave/UKC physics:** No `src/physics/resistance.rs` or
   `src/physics/ukc.rs` exists. Q-003 defers wave resistance; Q-004 does not
   resolve whether 5 m means chart depth or residual UKC and supplies no
   block coefficient/beam data for squat and heel calculations.
9. **No-safe-route response discrepancy:** Synchronous routing returns a
   status-shaped result. Async routing serializes it as a failed job with
   `ERR_NO_SAFE_ROUTE_FOUND`. The SRS asks for an error and heave-to
   recommendation, but no heave-to recommendation is generated.
10. **External-service test coverage:** Redis tests were ignored and the
   PostGIS Python test skipped in this run because test DSNs were absent.
   Earlier progress records successful live-service runs, but no current-run
   evidence is available.
11. **Minimum compiler:** `Cargo.toml` declares Rust 1.80, but the current
    validation used Rust 1.100. MSRV compatibility remains unverified by this
    audit.
12. **SI type enforcement:** Internal naming and boundary conversions are
    largely consistent with SI, but most structures hold primitive `f64`
    rather than the unit wrapper types; correctness relies on names and code
    review.
13. **No assumption annotations:** There are no `ASSUMPTION [Q-XXX]` comments
    in source. This is consistent with Q-004 explicitly blocking STEP-12 rather
    than implementing speculative UKC physics.

## Analyst questions and assumptions

| Question | Status | Audit interpretation |
|---|---|---|
| Q-001 — coastline simplification tolerance | **CLOSED** | 100 m default is used by `spatial_etl.py`; the CLI permits override. |
| Q-002 — VPP input formats | **CLOSED** | `.pol`, ORC JSON, and normalized/matrix CSV parsing are implemented; no fallback assumption is recorded. |
| Q-003 — wave resistance in MVP | **CLOSED** | Explicitly excluded. No wave-resistance speed reduction is implemented. |
| Q-004 — 5 m threshold and vessel profile for UKC | **OPEN / BLOCKING** | It is unclear whether 5 m is bathymetric depth or residual UKC; block coefficient, beam source, and squat/heel rejection policy are missing. STEP-12 must remain blocked. |

No undocumented UKC assumption was added. The implemented leeway formula and
synthetic polar/weather fixtures are documented as code behavior, not asserted
as validated real-vessel physics.

## Recommended next actions

1. Clarify or revise the WBS 5 ms weather-query target, then profile and
   optimize against the agreed, repeatable benchmark.
2. Resolve Q-004 before implementing depth, squat, tide, or heel-clearance
   routing.
3. Wire H3 corridor constraints, blended weather sources, ensemble/departure
   workflows, and NMEA event consumption only when their service-level
   contracts are defined.
4. Add representative datasets and automated P95/concurrency, gRPC
   first-chunk, and multi-process memory tests before claiming SRS SLA
   compliance.
5. Run the Redis and PostGIS integration suites in CI and build with the
   declared Rust 1.80 MSRV.
