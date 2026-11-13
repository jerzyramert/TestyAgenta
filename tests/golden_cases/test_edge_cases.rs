use sailboat_routing_engine::core::isochrone::{RouteStatus, RoutingConfig};
use sailboat_routing_engine::error::DomainError;
use sailboat_routing_engine::spatial::rtree_index::SpatialIndex;

use super::common::{
    circular_obstacle, config, empty_spatial_index, engine, motor_config, north_wind, point, polar,
    request, weather, ConstantClimatology, ConstantWeather, NAUTICAL_MILE_M,
};

#[test]
fn test_edge_cases() {
    let start = point(0.0, 0.0);
    let island = circular_obstacle(0.0, 0.0, NAUTICAL_MILE_M);
    let land_index = SpatialIndex::new([island]);
    let snapped_route = engine()
        .calculate_route(
            request(start, point(0.0, 0.3)),
            config(),
            &weather(0.0, -15.0),
            None,
            &land_index,
            &polar(),
        )
        .expect("route from a point one nautical mile inland is snapped");
    assert_eq!(snapped_route.status, RouteStatus::Completed);
    assert_ne!(snapped_route.waypoints[0].point, start);
    assert!(!land_index.contains_obstacle(snapped_route.waypoints[0].point));

    let distant_land = SpatialIndex::new([circular_obstacle(0.0, 0.0, 6.0 * NAUTICAL_MILE_M)]);
    let snap_error = engine()
        .calculate_route(
            request(start, point(0.0, 0.3)),
            config(),
            &weather(0.0, -15.0),
            None,
            &distant_land,
            &polar(),
        )
        .unwrap_err();
    assert_eq!(snap_error, DomainError::InvalidStartCoord);

    let storm_route = engine()
        .calculate_route(
            request(start, point(0.1, 0.0)),
            config(),
            &weather(0.0, -60.0),
            None,
            &empty_spatial_index(),
            &polar(),
        )
        .expect("storm blockade is represented as a route status");
    assert_eq!(storm_route.status, RouteStatus::NoSafeRouteFound);
    assert!(!storm_route.partial_isochrones.is_empty());

    let forecast = ConstantWeather {
        sample: north_wind().sample,
        bounds: Some((0, 3_600)),
    };
    let climatology = ConstantClimatology(forecast.sample);
    let fallback_route = engine()
        .calculate_route(
            request(start, point(0.3, 0.0)),
            RoutingConfig { ..config() },
            &forecast,
            Some(&climatology),
            &empty_spatial_index(),
            &polar(),
        )
        .expect("climatology covers forecast exhaustion");
    assert_eq!(fallback_route.status, RouteStatus::Completed);
    assert!(fallback_route.warning_climatology_used);

    let mut motor_routing_config = config();
    motor_routing_config.initial_fuel_l = 10.0;
    motor_routing_config.motor_sailing = Some(motor_config());
    let motor_route = engine()
        .calculate_route(
            request(start, point(0.0, 0.1)),
            motor_routing_config,
            &weather(0.0, 0.0),
            None,
            &empty_spatial_index(),
            &polar(),
        )
        .expect("motor-sailing route works without usable wind propulsion");
    assert_eq!(motor_route.status, RouteStatus::Completed);
    assert!(motor_route
        .waypoints
        .iter()
        .any(|waypoint| waypoint.is_motoring));
    assert!(motor_route
        .waypoints
        .windows(2)
        .all(|pair| pair[0].fuel_remaining_l >= pair[1].fuel_remaining_l));
}
