use std::str::FromStr;

use geo::{Geometry, Intersects, Line, Polygon};
use geojson::GeoJson;
use postgres::{Client, NoTls};
use rstar::{PointDistance, RTree, RTreeObject, AABB};

use crate::error::DomainError;
use crate::types::GeoPoint;

use super::geodesy::project_point;

const SNAP_SEARCH_STEP_M: f64 = 50.0;
const SNAP_BEARING_COUNT: usize = 72;

#[derive(Clone)]
struct IndexedObstacle {
    polygon: Polygon<f64>,
    envelope: AABB<[f64; 2]>,
}

impl IndexedObstacle {
    fn new(polygon: Polygon<f64>) -> Self {
        let mut min_lon = f64::INFINITY;
        let mut min_lat = f64::INFINITY;
        let mut max_lon = f64::NEG_INFINITY;
        let mut max_lat = f64::NEG_INFINITY;
        for coordinate in polygon.exterior().coords() {
            min_lon = min_lon.min(coordinate.x);
            min_lat = min_lat.min(coordinate.y);
            max_lon = max_lon.max(coordinate.x);
            max_lat = max_lat.max(coordinate.y);
        }
        Self {
            polygon,
            envelope: AABB::from_corners([min_lon, min_lat], [max_lon, max_lat]),
        }
    }
}

impl RTreeObject for IndexedObstacle {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

impl PointDistance for IndexedObstacle {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let lower = self.envelope.lower();
        let upper = self.envelope.upper();
        let delta_x = if point[0] < lower[0] {
            lower[0] - point[0]
        } else if point[0] > upper[0] {
            point[0] - upper[0]
        } else {
            0.0
        };
        let delta_y = if point[1] < lower[1] {
            lower[1] - point[1]
        } else if point[1] > upper[1] {
            point[1] - upper[1]
        } else {
            0.0
        };
        delta_x.mul_add(delta_x, delta_y * delta_y)
    }
}

pub struct SpatialIndex {
    obstacles: RTree<IndexedObstacle>,
}

impl SpatialIndex {
    pub fn new(polygons: impl IntoIterator<Item = Polygon<f64>>) -> Self {
        Self {
            obstacles: RTree::bulk_load(polygons.into_iter().map(IndexedObstacle::new).collect()),
        }
    }

    pub fn load_postgis(database_url: &str, valid_at_unix: i64) -> Result<Self, DomainError> {
        let mut client = Client::connect(database_url, NoTls)
            .map_err(|error| DomainError::Postgis(error.to_string()))?;
        let rows = client
            .query(
                r#"
            SELECT ST_AsGeoJSON(geom)
            FROM coastlines
            WHERE active_from <= to_timestamp($1)
              AND (active_to IS NULL OR active_to > to_timestamp($1))
            UNION ALL
            SELECT ST_AsGeoJSON(geom)
            FROM no_go_zones
            WHERE active_from <= to_timestamp($1)
              AND (active_to IS NULL OR active_to > to_timestamp($1))
            "#,
                &[&(valid_at_unix as f64)],
            )
            .map_err(|error| DomainError::Postgis(error.to_string()))?;

        let mut polygons = Vec::new();
        for row in rows {
            let geometry_json: String = row.get(0);
            let geojson = GeoJson::from_str(&geometry_json)
                .map_err(|error| DomainError::InvalidPostgisGeometry(error.to_string()))?;
            let GeoJson::Geometry(geometry) = geojson else {
                return Err(DomainError::InvalidPostgisGeometry(
                    "expected a GeoJSON geometry".to_owned(),
                ));
            };
            let geometry: Geometry<f64> =
                geometry.try_into().map_err(|error: geojson::Error| {
                    DomainError::InvalidPostgisGeometry(error.to_string())
                })?;
            match geometry {
                Geometry::Polygon(polygon) => polygons.push(polygon),
                Geometry::MultiPolygon(multi_polygon) => polygons.extend(multi_polygon.0),
                _ => {
                    return Err(DomainError::InvalidPostgisGeometry(
                        "expected polygonal coastline or no-go geometry".to_owned(),
                    ));
                }
            }
        }

        Ok(Self::new(polygons))
    }

    pub fn intersects_obstacle(&self, start: GeoPoint, end: GeoPoint) -> bool {
        let (first_segment, second_segment) = antimeridian_safe_segments(start, end);
        if self.intersects_segment(&first_segment) {
            return true;
        }
        second_segment.is_some_and(|segment| self.intersects_segment(&segment))
    }

    pub fn contains_obstacle(&self, point: GeoPoint) -> bool {
        let coordinate = [point.longitude(), point.latitude()];
        let envelope = AABB::from_point(coordinate);
        self.obstacles
            .locate_in_envelope_intersecting(&envelope)
            .any(|obstacle| {
                obstacle
                    .polygon
                    .intersects(&geo::Point::new(coordinate[0], coordinate[1]))
            })
    }

    pub fn snap_to_water(
        &self,
        point: GeoPoint,
        max_radius_m: f64,
    ) -> Result<GeoPoint, DomainError> {
        if !max_radius_m.is_finite() || max_radius_m < 0.0 {
            return Err(DomainError::InvalidSnapRadius);
        }
        if !self.contains_obstacle(point) {
            return Ok(point);
        }

        let bearing_step = std::f64::consts::TAU / SNAP_BEARING_COUNT as f64;
        let starting_bearing = self.nearest_obstacle_bearing(point);
        let mut radius_m = SNAP_SEARCH_STEP_M;
        while radius_m < max_radius_m {
            if let Some(water_point) =
                self.water_point_on_ring(point, radius_m, bearing_step, starting_bearing)?
            {
                return Ok(water_point);
            }
            radius_m += SNAP_SEARCH_STEP_M;
        }
        if max_radius_m > 0.0 {
            if let Some(water_point) =
                self.water_point_on_ring(point, max_radius_m, bearing_step, starting_bearing)?
            {
                return Ok(water_point);
            }
        }

        Err(DomainError::InvalidStartCoord)
    }

    fn water_point_on_ring(
        &self,
        origin: GeoPoint,
        radius_m: f64,
        bearing_step: f64,
        starting_bearing: f64,
    ) -> Result<Option<GeoPoint>, DomainError> {
        for index in 0..SNAP_BEARING_COUNT {
            let bearing =
                (starting_bearing + index as f64 * bearing_step).rem_euclid(std::f64::consts::TAU);
            let candidate = project_point(origin, bearing, radius_m)?;
            if !self.contains_obstacle(candidate) {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn nearest_obstacle_bearing(&self, point: GeoPoint) -> f64 {
        let query = [point.longitude(), point.latitude()];
        let Some(obstacle) = self.obstacles.nearest_neighbor(&query) else {
            return 0.0;
        };
        let lower = obstacle.envelope.lower();
        let upper = obstacle.envelope.upper();
        let center_lon = (lower[0] + upper[0]) / 2.0;
        let center_lat = (lower[1] + upper[1]) / 2.0;
        let delta_lon = (center_lon - point.longitude() + 180.0).rem_euclid(360.0) - 180.0;
        ((delta_lon.to_radians() * point.latitude().to_radians().cos())
            .atan2((center_lat - point.latitude()).to_radians())
            + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
    }

    fn intersects_segment(&self, segment: &Line<f64>) -> bool {
        let envelope = AABB::from_corners(
            [
                segment.start.x.min(segment.end.x),
                segment.start.y.min(segment.end.y),
            ],
            [
                segment.start.x.max(segment.end.x),
                segment.start.y.max(segment.end.y),
            ],
        );
        self.obstacles
            .locate_in_envelope_intersecting(&envelope)
            .any(|obstacle| segment.intersects(&obstacle.polygon))
    }
}

fn antimeridian_safe_segments(start: GeoPoint, end: GeoPoint) -> (Line<f64>, Option<Line<f64>>) {
    let raw_delta = end.longitude() - start.longitude();
    if raw_delta.abs() <= 180.0 {
        return (
            Line::new(
                (start.longitude(), start.latitude()),
                (end.longitude(), end.latitude()),
            ),
            None,
        );
    }

    let shortest_delta = (raw_delta + 180.0).rem_euclid(360.0) - 180.0;
    let edge_longitude = if start.longitude() > 0.0 {
        180.0
    } else {
        -180.0
    };
    let fraction = (edge_longitude - start.longitude()) / shortest_delta;
    let crossing_latitude = start.latitude() + (end.latitude() - start.latitude()) * fraction;
    let opposite_edge = -edge_longitude;

    (
        Line::new(
            (start.longitude(), start.latitude()),
            (edge_longitude, crossing_latitude),
        ),
        Some(Line::new(
            (opposite_edge, crossing_latitude),
            (end.longitude(), end.latitude()),
        )),
    )
}

#[cfg(test)]
mod tests {
    use std::env;

    use geo::{polygon, Polygon};
    use postgres::{Client, NoTls};

    use crate::error::DomainError;
    use crate::types::GeoPoint;

    use super::SpatialIndex;

    fn rectangle(min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Polygon<f64> {
        polygon![
            (x: min_lon, y: min_lat),
            (x: max_lon, y: min_lat),
            (x: max_lon, y: max_lat),
            (x: min_lon, y: max_lat),
            (x: min_lon, y: min_lat),
        ]
    }

    #[test]
    fn detects_intersecting_segments_and_ignores_clear_water() {
        let index = SpatialIndex::new([rectangle(-0.1, -0.1, 0.1, 0.1)]);
        let start = GeoPoint::new(0.0, -0.2).unwrap();
        let end = GeoPoint::new(0.0, 0.2).unwrap();
        let clear_end = GeoPoint::new(0.6, 0.3).unwrap();

        assert!(index.intersects_obstacle(start, end));
        assert!(!index.intersects_obstacle(start, clear_end));
    }

    #[test]
    fn splits_segments_crossing_the_antimeridian_for_collision_queries() {
        let index = SpatialIndex::new([
            rectangle(179.8, -0.1, 180.0, 0.1),
            rectangle(-180.0, -0.1, -179.8, 0.1),
        ]);
        let start = GeoPoint::new(0.0, 179.5).unwrap();
        let end = GeoPoint::new(0.0, -179.5).unwrap();

        assert!(index.intersects_obstacle(start, end));
    }

    #[test]
    fn snaps_to_nearest_sampled_water_ring_and_rejects_distant_land() {
        let coastal_land = SpatialIndex::new([rectangle(-0.003, -0.003, 0.003, 0.003)]);
        let inland_start = GeoPoint::new(0.0, 0.0).unwrap();
        let snapped = coastal_land.snap_to_water(inland_start, 3_704.0).unwrap();
        let (distance, _) =
            super::super::geodesy::distance_and_bearing(inland_start, snapped).unwrap();
        assert!(distance <= 500.0);
        assert!(!coastal_land.contains_obstacle(snapped));

        let large_land = SpatialIndex::new([rectangle(-0.2, -0.2, 0.2, 0.2)]);
        assert_eq!(
            large_land.snap_to_water(inland_start, 3_704.0),
            Err(DomainError::InvalidStartCoord)
        );
    }

    #[test]
    fn loads_active_postgis_coastlines_into_the_rtree() {
        let Ok(database_url) = env::var("POSTGIS_TEST_DSN") else {
            return;
        };
        let valid_at_unix = 1_800_000_000_i64;
        let mut client = Client::connect(&database_url, NoTls).unwrap();
        let row = client
            .query_one(
                r#"
                INSERT INTO coastlines (source, geom, active_from)
                VALUES (
                    'rtree-index-test',
                    ST_GeomFromText(
                        'MULTIPOLYGON(((10 10, 10.1 10, 10.1 10.1, 10 10.1, 10 10)))',
                        4326
                    ),
                    to_timestamp($1)
                )
                RETURNING id
                "#,
                &[&(valid_at_unix as f64)],
            )
            .unwrap();
        let inserted_id: i64 = row.get(0);

        let result = SpatialIndex::load_postgis(&database_url, valid_at_unix);
        client
            .execute("DELETE FROM coastlines WHERE id = $1", &[&inserted_id])
            .unwrap();
        let index = result.unwrap();

        assert!(index.intersects_obstacle(
            GeoPoint::new(9.9, 10.05).unwrap(),
            GeoPoint::new(10.2, 10.05).unwrap(),
        ));
    }
}
