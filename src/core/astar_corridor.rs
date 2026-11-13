use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use geo::{BooleanOps, Coord, LineString, MultiPolygon, Polygon};
use h3o::{CellIndex, LatLng, Resolution};

use crate::error::DomainError;
use crate::spatial::geodesy::distance_and_bearing;
use crate::spatial::rtree_index::SpatialIndex;
use crate::types::GeoPoint;

#[derive(Debug, Clone)]
pub struct AStarCorridor {
    cells: Vec<CellIndex>,
    polygon: MultiPolygon<f64>,
    resolution: Resolution,
    longitude_origin_deg: f64,
}

impl AStarCorridor {
    pub fn cells(&self) -> &[CellIndex] {
        &self.cells
    }

    pub fn polygon(&self) -> &MultiPolygon<f64> {
        &self.polygon
    }

    pub fn contains(&self, point: GeoPoint) -> Result<bool, DomainError> {
        Ok(self.cells.contains(&cell_for(point, self.resolution)?))
    }

    pub fn longitude_origin_deg(&self) -> f64 {
        self.longitude_origin_deg
    }
}

#[derive(Debug, Clone, Copy)]
struct OpenEntry {
    estimated_total_s: f64,
    elapsed_s: f64,
    cell: CellIndex,
    sequence: u64,
}

impl PartialEq for OpenEntry {
    fn eq(&self, other: &Self) -> bool {
        self.estimated_total_s.total_cmp(&other.estimated_total_s) == Ordering::Equal
            && self.sequence == other.sequence
    }
}

impl Eq for OpenEntry {}

impl PartialOrd for OpenEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OpenEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimated_total_s
            .total_cmp(&self.estimated_total_s)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

pub fn build_corridor(
    start: GeoPoint,
    target: GeoPoint,
    resolution: Resolution,
    max_speed_mps: f64,
    max_expanded_cells: usize,
    spatial: &SpatialIndex,
) -> Result<AStarCorridor, DomainError> {
    if !max_speed_mps.is_finite() || max_speed_mps <= 0.0 || max_expanded_cells == 0 {
        return Err(DomainError::InvalidRouteConfig(
            "A* maximum speed and expansion limit must be positive".to_owned(),
        ));
    }
    if spatial.contains_obstacle(start) || spatial.contains_obstacle(target) {
        return Err(DomainError::NoSafeRouteFound);
    }

    let start_cell = cell_for(start, resolution)?;
    let target_cell = cell_for(target, resolution)?;
    let start_center = center(start_cell)?;
    if spatial.intersects_obstacle(start, start_center) {
        return Err(DomainError::NoSafeRouteFound);
    }

    let mut open = BinaryHeap::new();
    let mut sequence = 0_u64;
    open.push(OpenEntry {
        estimated_total_s: heuristic(start_center, target, max_speed_mps)?,
        elapsed_s: 0.0,
        cell: start_cell,
        sequence,
    });

    let mut elapsed_by_cell = HashMap::from([(start_cell, 0.0)]);
    let mut parent_by_cell = HashMap::new();
    let mut expanded_cells = 0;
    let mut goal_reached = false;

    while let Some(entry) = open.pop() {
        if elapsed_by_cell.get(&entry.cell) != Some(&entry.elapsed_s) {
            continue;
        }
        expanded_cells += 1;
        if expanded_cells > max_expanded_cells {
            return Err(DomainError::NoSafeRouteFound);
        }
        let current_center = center(entry.cell)?;
        if entry.cell == target_cell {
            if !spatial.intersects_obstacle(current_center, target) {
                goal_reached = true;
                break;
            }
            continue;
        }

        let mut neighbors: Vec<CellIndex> = entry.cell.grid_disk(1);
        neighbors.sort_unstable_by_key(|cell| u64::from(*cell));
        for neighbor in neighbors {
            if neighbor == entry.cell {
                continue;
            }
            let neighbor_center = center(neighbor)?;
            if spatial.contains_obstacle(neighbor_center)
                || spatial.intersects_obstacle(current_center, neighbor_center)
            {
                continue;
            }

            let (distance_m, _) = distance_and_bearing(current_center, neighbor_center)?;
            let tentative_elapsed_s = entry.elapsed_s + distance_m / max_speed_mps;
            if elapsed_by_cell
                .get(&neighbor)
                .is_some_and(|known| tentative_elapsed_s >= *known)
            {
                continue;
            }

            let estimate = tentative_elapsed_s + heuristic(neighbor_center, target, max_speed_mps)?;
            elapsed_by_cell.insert(neighbor, tentative_elapsed_s);
            parent_by_cell.insert(neighbor, entry.cell);
            sequence = sequence.checked_add(1).ok_or_else(|| {
                DomainError::InvalidRouteConfig("A* queue sequence overflow".to_owned())
            })?;
            open.push(OpenEntry {
                estimated_total_s: estimate,
                elapsed_s: tentative_elapsed_s,
                cell: neighbor,
                sequence,
            });
        }
    }

    if !goal_reached {
        return Err(DomainError::NoSafeRouteFound);
    }

    let mut cells = vec![target_cell];
    let mut cell = target_cell;
    while cell != start_cell {
        cell = *parent_by_cell
            .get(&cell)
            .ok_or(DomainError::NoSafeRouteFound)?;
        cells.push(cell);
    }
    cells.reverse();

    let mut polygon = MultiPolygon(vec![cell_polygon(cells[0], start.longitude())?]);
    for cell in cells.iter().copied().skip(1) {
        polygon = polygon.union(&MultiPolygon(vec![cell_polygon(cell, start.longitude())?]));
    }

    Ok(AStarCorridor {
        cells,
        polygon,
        resolution,
        longitude_origin_deg: start.longitude(),
    })
}

fn cell_for(point: GeoPoint, resolution: Resolution) -> Result<CellIndex, DomainError> {
    let coordinate = LatLng::new(point.latitude(), point.longitude())
        .map_err(|error| DomainError::InvalidRouteConfig(error.to_string()))?;
    Ok(coordinate.to_cell(resolution))
}

fn center(cell: CellIndex) -> Result<GeoPoint, DomainError> {
    let coordinate = LatLng::from(cell);
    GeoPoint::new(coordinate.lat(), coordinate.lng())
}

fn heuristic(point: GeoPoint, target: GeoPoint, max_speed_mps: f64) -> Result<f64, DomainError> {
    Ok(distance_and_bearing(point, target)?.0 / max_speed_mps)
}

fn cell_polygon(cell: CellIndex, longitude_origin_deg: f64) -> Result<Polygon<f64>, DomainError> {
    let mut boundary: Vec<Coord<f64>> = cell
        .boundary()
        .iter()
        .map(|coordinate| Coord {
            x: unwrap_longitude(coordinate.lng(), longitude_origin_deg),
            y: coordinate.lat(),
        })
        .collect();
    let first = boundary
        .first()
        .copied()
        .ok_or_else(|| DomainError::InvalidRouteConfig("H3 cell has no boundary".to_owned()))?;
    boundary.push(first);
    Ok(Polygon::new(LineString::new(boundary), vec![]))
}

fn unwrap_longitude(longitude_deg: f64, origin_deg: f64) -> f64 {
    origin_deg + (longitude_deg - origin_deg + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use geo::{polygon, Area, MultiPolygon};
    use h3o::Resolution;

    use crate::error::DomainError;
    use crate::spatial::rtree_index::SpatialIndex;
    use crate::types::GeoPoint;

    use super::build_corridor;

    #[test]
    fn builds_a_deterministic_h3_corridor_and_polygon() {
        let start = GeoPoint::new(0.0, 0.0).unwrap();
        let target = GeoPoint::new(0.02, 0.02).unwrap();
        let spatial = SpatialIndex::new([]);

        let first = build_corridor(start, target, Resolution::Nine, 10.0, 2_000, &spatial).unwrap();
        let second =
            build_corridor(start, target, Resolution::Nine, 10.0, 2_000, &spatial).unwrap();

        assert_eq!(first.cells(), second.cells());
        assert!(first.contains(start).unwrap());
        assert!(first.contains(target).unwrap());
        assert!(!first.polygon().0.is_empty());
        assert!(first
            .polygon()
            .0
            .iter()
            .all(|polygon| polygon.unsigned_area() > 0.0));
    }

    #[test]
    fn finds_a_passage_through_a_narrow_water_corridor() {
        let start = GeoPoint::new(0.0, -0.03).unwrap();
        let target = GeoPoint::new(0.0, 0.03).unwrap();
        let land = MultiPolygon(vec![
            polygon![
                (x: -0.02, y: 0.004),
                (x: 0.02, y: 0.004),
                (x: 0.02, y: 0.04),
                (x: -0.02, y: 0.04),
                (x: -0.02, y: 0.004),
            ],
            polygon![
                (x: -0.02, y: -0.04),
                (x: 0.02, y: -0.04),
                (x: 0.02, y: -0.004),
                (x: -0.02, y: -0.004),
                (x: -0.02, y: -0.04),
            ],
        ]);
        let spatial = SpatialIndex::new(land.0);

        let corridor =
            build_corridor(start, target, Resolution::Nine, 10.0, 10_000, &spatial).unwrap();

        assert!(corridor.contains(start).unwrap());
        assert!(corridor.contains(target).unwrap());
        let centers: Vec<GeoPoint> = corridor
            .cells()
            .iter()
            .map(|cell| {
                let center = h3o::LatLng::from(*cell);
                GeoPoint::new(center.lat(), center.lng()).unwrap()
            })
            .collect();
        assert!(centers
            .windows(2)
            .all(|pair| !spatial.intersects_obstacle(pair[0], pair[1])));
        assert!(centers.iter().any(|point| point.latitude().abs() < 0.004));
    }

    #[test]
    fn rejects_invalid_search_limits_and_bounded_search_exhaustion() {
        let start = GeoPoint::new(0.0, 0.0).unwrap();
        let target = GeoPoint::new(0.1, 0.1).unwrap();
        let spatial = SpatialIndex::new([]);

        assert!(matches!(
            build_corridor(start, target, Resolution::Nine, 0.0, 10, &spatial),
            Err(DomainError::InvalidRouteConfig(_))
        ));
        assert!(matches!(
            build_corridor(start, target, Resolution::Nine, 10.0, 1, &spatial),
            Err(DomainError::NoSafeRouteFound)
        ));
    }
}
