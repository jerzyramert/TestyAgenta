use sailboat_routing_engine::core::isochrone::RouteStatus;
use sailboat_routing_engine::spatial::geodesy::distance_and_bearing;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;

use super::common::{config, engine, north_wind, point, request, upwind_polar};

#[test]
fn test_upwind_tacking() {
    let start = point(0.0, 0.0);
    let target = point(1.0, 0.0);
    let route = engine()
        .calculate_route(
            request(start, target),
            config(),
            &north_wind(),
            None,
            &SpatialIndex::new([]),
            &upwind_polar(),
        )
        .expect("upwind route calculation succeeds");

    assert_eq!(route.status, RouteStatus::Completed);
    assert!(
        distance_and_bearing(route.waypoints.last().unwrap().point, target)
            .unwrap()
            .0
            <= 0.5 * 1_852.0
    );
    let courses: Vec<f64> = route
        .waypoints
        .iter()
        .map(|waypoint| waypoint.course_rad.to_degrees())
        .filter(|course| (course - 42.0).abs() <= 2.0 || (course - 318.0).abs() <= 2.0)
        .collect();
    assert!(courses.len() > 2);
    assert!(courses
        .windows(2)
        .any(|pair| (pair[0] - pair[1]).abs() > 180.0));
    let unexpected_courses: Vec<f64> = route
        .waypoints
        .iter()
        .filter_map(|waypoint| {
            let course = waypoint.course_rad.to_degrees();
            (waypoint.sog_mps > 0.0 && (course - 42.0).abs() > 2.0 && (course - 318.0).abs() > 2.0)
                .then_some(course)
        })
        .collect();
    assert!(unexpected_courses.is_empty(), "{unexpected_courses:?}");
}
