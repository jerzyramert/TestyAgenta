use sailboat_routing_engine::core::isochrone::RouteStatus;
use sailboat_routing_engine::spatial::geodesy::distance_and_bearing;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;

use super::common::{engine, point, polar, request, weather, NAUTICAL_MILE_M};

#[test]
fn test_antimeridian() {
    let route = engine()
        .calculate_route(
            request(point(0.0, 179.5), point(0.0, -179.5)),
            super::common::config(),
            &weather(0.0, -12.0),
            None,
            &SpatialIndex::new([]),
            &polar(),
        )
        .expect("antimeridian route calculation succeeds");

    assert_eq!(route.status, RouteStatus::Completed);
    let distance_m: f64 = route
        .waypoints
        .windows(2)
        .map(|pair| {
            distance_and_bearing(pair[0].point, pair[1].point)
                .unwrap()
                .0
        })
        .sum();
    assert!(
        (distance_m - 60.0 * NAUTICAL_MILE_M).abs() <= 0.5 * NAUTICAL_MILE_M,
        "distance_m={distance_m}"
    );
    assert!(route
        .waypoints
        .windows(2)
        .any(|pair| (pair[0].point.longitude() - pair[1].point.longitude()).abs() > 180.0));
}
