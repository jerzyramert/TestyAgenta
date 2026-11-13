# Core Algorithms and Math

All router-internal distances and speeds use metres and metres per second,
angles use radians, and route/weather times use UTC Unix seconds. Public API
conversions are listed in the [API reference](./04_API_REFERENCE.md).

## Weather sampling

[`WeatherGrid4D`](../src/weather/interpolator.rs) is a three-axis
space-time interpolator (latitude, longitude, and time) over eight configured
scalar fields. For each scalar, it brackets the query on each axis and applies
the tensor-product linear weights at up to eight grid vertices:

$$
f(\phi,\lambda,t) =
\sum_{i,j,k \in \{0,1\}}
w^\phi_i w^\lambda_j w^t_k f_{ijk}.
$$

The axes can be ascending or descending for latitude/longitude; time must be
strictly increasing. Longitudes are normalized to `[-180°, 180°)`. A cyclic
longitude bracket is used when the stored axis spans approximately the globe.
Wind and current are stored/interpolated as U/V components. Wave direction is
interpolated as weighted sine/cosine components to avoid averaging across the
0/360-degree seam. Units declared in Zarr metadata are converted to SI while
loading the sample.

Forecast bounds are checked by the router. If a query exceeds those bounds,
the router uses a supplied `ClimatologySource` and marks the result; otherwise
the weather error propagates. This fallback is not active in the standard
server startup because [`main.rs`](../src/main.rs) sets `climatology: None`.

## Polar performance and no-sail behavior

[`Polar`](../src/physics/polar.rs) accepts `.pol`, ORC JSON, normalized CSV,
and explicitly selected matrix CSV. Polar input speed axes/cells are converted
from their declared units (the simple `.pol` and matrix CSV forms use knots);
TWA becomes radians. For each sail, boat speed is interpolated bilinearly
across TWS and TWA. Missing/invalid cells remain unreachable. Below the
minimum TWS grid value, speed scales quadratically with wind speed. The polar
logic uses the derived VMG beat angle as the upwind boundary: it returns zero
at or below 75% of that angle and ramps linearly to full speed at the beat
angle. Downwind of the derived VMG run angle, it tapers linearly to zero at
180 degrees. VMG beat/run values and a best-sail crossover matrix are derived
from the polar grid. These are deterministic interpolation rules, not a
calibrated hydrodynamic model.

Polar caching uses the Redis key `vpp:{vessel_id}`. The API validation path
parses a submitted polar, validates it, stores it, and returns maximum speed
and sail count.

## Wind-relative kinematics and motor sailing

[`compute_true_wind`](../src/physics/kinematics.rs) derives wind-from bearing
from U/V and subtracts heading, normalizing the relative angle to
`[-π, π)`. For heading $\psi$ and STW $v$, the sailing velocity components are

$$
u_s = v\sin\psi,\qquad v_s = v\cos\psi.
$$

The implemented leeway term is

$$
v_\ell = \frac{c_\ell\,\theta_{\text{heel}}}
                 {\max(v,0.25)^2},
\quad
u_\ell = v_\ell\cos\psi,\quad
v_\ell^{(N)}=-v_\ell\sin\psi,
$$

where the denominator floor is `0.25 m/s`. The leeway and current components
are added to the sailing components:

$$
u = u_s + u_\ell + u_{\text{current}},\quad
v_N = v_s + v_\ell^{(N)} + v_{\text{current}}.
$$

Then `SOG = hypot(u, v_N)` and `COG = atan2(u, v_N)` normalized to one turn.
This is the formula presently coded; its coefficient calibration and physical
accuracy for a real vessel are not validated by these synthetic tests.

When configured, motor-sailing activates only if SOG is strictly below the
threshold and the remaining fuel covers the interval's full consumption.
Consumption is `fuel_rate_lph * elapsed_seconds / 3600`. A motoring candidate
uses the configured fixed speed along heading and burns fuel. If fuel is
insufficient, the sailing velocity is retained; no engine-assisted edge is
created. Candidates with zero resulting SOG are discarded.

## Edge costs

[`evaluate_edge_cost`](../src/core/cost.rs) accumulates:

$$
C_{\text{edge}} =
\Delta t + P_{\text{tack}} + P_{\text{gybe}} + P_{\text{sail}}
+ w_f F_{\text{used}}
+ w_h\max(H_s-H_{\text{comfort}},0).
$$

Tacks are detected when signed TWA crosses the wind axis without crossing the
downwind seam; gybes cross the downwind seam. Fuel and wave terms are weighted
cost penalties. Wave height also participates in route pruning, but it does
not reduce polar STW: wave-resistance physics is explicitly out of scope.

## Modified isochrone search

[`RouteEngine::calculate_route_with_progress`](../src/core/isochrone.rs)
operates as follows:

1. Validate route limits and the initial sail index; snap start and target
   points out of indexed obstacles within a 3,704 m (2 NM) radius.
2. For each frontier node, sample weather once. Prune the node if TWS or wave
   height exceeds configured limits.
3. Sample headings at the configured angular interval. For each heading,
   calculate true wind, select the best reachable sail, compose current/leeway
   velocity, optionally evaluate motoring, and reject zero-speed or
   obstacle-intersecting edges.
4. Rank non-terminal candidates into 360 one-degree bearings-to-target
   buckets. Each bucket retains the candidate minimizing accumulated cost plus
   straight-line distance divided by maximum possible SOG.
5. If a candidate enters the target capture radius (maximum SOG times the time
   step), test the final segment for collision and select the lowest-cost
   terminal route. Otherwise propagate the reduced frontier.
6. Stop when a route is found, the frontier collapses, or `max_steps` is
   reached. The last two cases return `RouteStatus::NoSafeRouteFound` with
   partial isochrones.

The frontier is bounded to at most 360 selected candidates per step, but the
search still evaluates the configured number of headings for every node in
that frontier. Heading intervals are validated and capped at 36,000 samples
per node.

## Spatial calculations

[`project_point`](../src/spatial/geodesy.rs) and
[`distance_and_bearing`](../src/spatial/geodesy.rs) implement iterative
Vincenty direct/inverse calculations on WGS84, with a 100-iteration limit and
`1e-12` convergence tolerance. The inverse calculation normalizes longitude
difference across the antimeridian. Non-convergence returns a domain error;
there is no spherical fallback.

[`SpatialIndex`](../src/spatial/rtree_index.rs) holds polygon obstacles in an
in-memory R-tree. Segment checks query an axis-aligned bounding box, then test
candidate polygons with exact planar geometry intersection. Antimeridian
crossings are split into two segments. This is not a geodesic polygon engine.

Snap-to-water only activates when the input point intersects an indexed
obstacle. It searches rings at 50 m intervals in 72 bearings, starting near
the nearest obstacle's bearing, until it finds a point outside all indexed
obstacles or exhausts the allowed radius. Water depth is not considered.

## H3 corridor, weather blending, and risk helpers

[`build_corridor`](../src/core/astar_corridor.rs) runs bounded deterministic
A* over neighboring H3 cells. Edge cost is geodesic distance divided by
`max_speed_mps`; the heuristic uses the same lower-bound travel-time form.
Obstacle intersections prune transitions, expanded cells are capped, and the
result includes both the cell path and a unioned polygon. This module is not
currently called by the regular route engine or API.

[`regional_weight`](../src/weather/blender.rs) and
[`blend_weather_samples`](../src/weather/blender.rs) provide a bounded
sigmoidal regional weight and blend scalar fields; directional fields are
combined circularly. They are standalone helpers and are not composed into
`WeatherGrid4D` in server startup.

For signed boundary distance $d$ and positive transition scale $s$, the
regional weight and scalar blend are:

$$
w_r = \frac{1}{1+\exp(-d/s)},\qquad
x = (1-w_r)x_{\text{global}} + w_r x_{\text{regional}}.
$$

The wave-direction blend applies the same weights to the direction's sine and
cosine components, then recovers the angle with `atan2`.

[`optimize_departures`](../src/core/ensemble.rs) evaluates unique requested
departure timestamps in parallel and chooses the completed route with minimum
terminal cost, breaking ties by departure timestamp.
[`evaluate_ensemble`](../src/core/ensemble.rs) evaluates members in parallel.
`cvar_tail_risk` averages the fractional worst-loss tail at the requested
confidence; infeasible members are represented as infinite loss. These
utilities are not exposed as network API methods or automatically supplied
with NWP ensemble data.

For confidence $\alpha$ and sorted losses $L_{(1)}\le \dots \le L_{(N)}$, the
implementation averages the largest $N(1-\alpha)$ losses, including a
fractional boundary member when that count is not an integer:

$$
\operatorname{CVaR}_{\alpha}
= \frac{\sum_{i=1}^{N} q_i L_{(N-i+1)}}{N(1-\alpha)},
\qquad
q_i = \min\!\left(1,\max\!\left(0,N(1-\alpha)-(i-1)\right)\right).
$$

## Deferred physics

There is no implementation of ISO 15016/Kwon wave resistance, dynamic UKC,
Barrass squat, heel-clearance, or tide corrections. Q-003 explicitly excludes
wave resistance from the MVP. Q-004 leaves the meaning of the 5 m safe-depth
threshold and the required hull parameters unresolved. Do not infer safety
clearance from the current route output.

## Public Rust library surface

The crate root exports `api`, `core`, `error`, `physics`, `spatial`, `types`,
and `weather` modules. Principal public types and entry points are:

| Module | Public surface |
|---|---|
| `types` | `MetersPerSecond`, `Knots`, `Radians`, `Degrees`, `GeoPoint::new`, `WeatherSample`; conversion implementations are provided for knots↔m/s and degrees↔radians. |
| `physics::kinematics` | `TrueWind`, `Velocity`, `MotorSailingConfig`, `MotorSailingOutcome`; `compute_true_wind`, `compose_velocity_vectors`, `evaluate_motor_sailing`. |
| `physics::polar` | `CsvFormat`, `VmgValue`, `VmgSample`, `SailPolar`, `Polar`, `PolarPerformance`, `PolarCache`; parse/validate, axes and sail accessors, speed lookup, `best_speed`, VMG/crossover access, cache-key and Redis load/store methods. |
| `weather::interpolator` | `WeatherVariables::new`, `WeatherGrid4D::open_zarr`, `get_4d_point`, and `time_bounds`. |
| `weather::zarr_mmap` | `MmapZarrArray::open`, `shape`, `dimensions`, `units`, `value_f64`, and `value_i64`. |
| `weather::blender` | `regional_weight` and `blend_weather_samples`. |
| `spatial::geodesy` | `project_point` and `distance_and_bearing`. |
| `spatial::rtree_index` | `SpatialIndex::new`, `load_postgis`, `intersects_obstacle`, `contains_obstacle`, and `snap_to_water`. |
| `core::isochrone` | `WeatherSource`, `ClimatologySource`, `RouteStatus`, `Waypoint`, `RouteResult`, `RoutingConfig`, `RouteRequest`, `RouteEngine::new`, `calculate_route`, and `calculate_route_with_progress`. |
| `core::cost` | `EdgeCostConfig`, `EdgeCostInput`, `ManeuverChanges`, `EdgeCostBreakdown`, `detect_maneuvers`, and `evaluate_edge_cost`. |
| `core::astar_corridor` | `AStarCorridor` accessors and `build_corridor`. |
| `core::ensemble` | `DepartureCandidate`, `DepartureOptimization`, `EnsembleEvaluation`, `optimize_departures`, `evaluate_ensemble`, and `cvar_tail_risk`. |

These are library APIs; the network interfaces expose only the operations
described in the [API reference](./04_API_REFERENCE.md).
