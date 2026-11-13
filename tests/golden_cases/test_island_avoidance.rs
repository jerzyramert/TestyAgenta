use sailboat_routing_engine::core::isochrone::RouteStatus;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;

use super::common::{
    circular_obstacle, config, engine, point, polar, request, weather, NAUTICAL_MILE_M,
};

#[test]
fn test_island_avoidance() {
    let island = circular_obstacle(0.5, 0.0, 10.0 * NAUTICAL_MILE_M + 50.0);
    let spatial = SpatialIndex::new([island]);
    let route = engine()
        .calculate_route(
            request(point(0.0, 0.0), point(1.0, 0.0)),
            config(),
            &weather(15.0, 0.0),
            None,
            &spatial,
            &polar(),
        )
        .expect("island-avoidance route calculation succeeds");

    assert_eq!(route.status, RouteStatus::Completed);
    assert_eq!(route.waypoints.last().unwrap().point, point(1.0, 0.0));
    assert!(route
        .waypoints
        .windows(2)
        .all(|pair| !spatial.intersects_obstacle(pair[0].point, pair[1].point)));
}
