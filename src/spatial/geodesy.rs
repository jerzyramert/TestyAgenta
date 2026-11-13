use std::f64::consts::PI;

use crate::error::DomainError;
use crate::types::GeoPoint;

const WGS84_SEMI_MAJOR_AXIS_M: f64 = 6_378_137.0;
const WGS84_FLATTENING: f64 = 1.0 / 298.257_223_563;
const WGS84_SEMI_MINOR_AXIS_M: f64 = WGS84_SEMI_MAJOR_AXIS_M * (1.0 - WGS84_FLATTENING);
const MAX_ITERATIONS: usize = 100;
const CONVERGENCE_TOLERANCE: f64 = 1e-12;

pub fn project_point(
    origin: GeoPoint,
    bearing_rad: f64,
    distance_m: f64,
) -> Result<GeoPoint, DomainError> {
    if !distance_m.is_finite() || distance_m < 0.0 {
        return Err(DomainError::InvalidDistance);
    }
    if !bearing_rad.is_finite() {
        return Err(DomainError::InvalidBearing);
    }

    let latitude_1 = origin.latitude().to_radians();
    let longitude_1 = origin.longitude().to_radians();
    let sin_bearing = bearing_rad.sin();
    let cos_bearing = bearing_rad.cos();
    let tan_reduced_latitude_1 = (1.0 - WGS84_FLATTENING) * latitude_1.tan();
    let cos_reduced_latitude_1 = 1.0 / (1.0 + tan_reduced_latitude_1.powi(2)).sqrt();
    let sin_reduced_latitude_1 = tan_reduced_latitude_1 * cos_reduced_latitude_1;
    let sigma_1 = tan_reduced_latitude_1.atan2(cos_bearing);
    let sin_alpha = cos_reduced_latitude_1 * sin_bearing;
    let cos_squared_alpha = 1.0 - sin_alpha.powi(2);
    let u_squared = cos_squared_alpha
        * (WGS84_SEMI_MAJOR_AXIS_M.powi(2) - WGS84_SEMI_MINOR_AXIS_M.powi(2))
        / WGS84_SEMI_MINOR_AXIS_M.powi(2);
    let coefficient_a = 1.0
        + u_squared / 16_384.0
            * (4_096.0 + u_squared * (-768.0 + u_squared * (320.0 - 175.0 * u_squared)));
    let coefficient_b = u_squared / 1_024.0
        * (256.0 + u_squared * (-128.0 + u_squared * (74.0 - 47.0 * u_squared)));

    let mut sigma = distance_m / (WGS84_SEMI_MINOR_AXIS_M * coefficient_a);
    let mut converged = false;
    for _ in 0..MAX_ITERATIONS {
        let two_sigma_midpoint = 2.0 * sigma_1 + sigma;
        let delta_sigma = coefficient_b
            * sigma.sin()
            * (two_sigma_midpoint.cos()
                + coefficient_b / 4.0
                    * (sigma.cos() * (-1.0 + 2.0 * two_sigma_midpoint.cos().powi(2))
                        - coefficient_b / 6.0
                            * two_sigma_midpoint.cos()
                            * (-3.0 + 4.0 * sigma.sin().powi(2))
                            * (-3.0 + 4.0 * two_sigma_midpoint.cos().powi(2))));
        let next_sigma = distance_m / (WGS84_SEMI_MINOR_AXIS_M * coefficient_a) + delta_sigma;
        if (next_sigma - sigma).abs() <= CONVERGENCE_TOLERANCE {
            sigma = next_sigma;
            converged = true;
            break;
        }
        sigma = next_sigma;
    }
    if !converged {
        return Err(DomainError::GeodesyDidNotConverge);
    }

    let sin_sigma = sigma.sin();
    let cos_sigma = sigma.cos();
    let two_sigma_midpoint = 2.0 * sigma_1 + sigma;
    let latitude_2 = (sin_reduced_latitude_1 * cos_sigma
        + cos_reduced_latitude_1 * sin_sigma * cos_bearing)
        .atan2(
            (1.0 - WGS84_FLATTENING)
                * (sin_alpha.powi(2)
                    + (sin_reduced_latitude_1 * sin_sigma
                        - cos_reduced_latitude_1 * cos_sigma * cos_bearing)
                        .powi(2))
                .sqrt(),
        );
    let lambda = (sin_sigma * sin_bearing).atan2(
        cos_reduced_latitude_1 * cos_sigma - sin_reduced_latitude_1 * sin_sigma * cos_bearing,
    );
    let coefficient_c = WGS84_FLATTENING / 16.0
        * cos_squared_alpha
        * (4.0 + WGS84_FLATTENING * (4.0 - 3.0 * cos_squared_alpha));
    let longitude_delta = lambda
        - (1.0 - coefficient_c)
            * WGS84_FLATTENING
            * sin_alpha
            * (sigma
                + coefficient_c
                    * sin_sigma
                    * (two_sigma_midpoint.cos()
                        + coefficient_c
                            * cos_sigma
                            * (-1.0 + 2.0 * two_sigma_midpoint.cos().powi(2))));
    let longitude_2 = (longitude_1 + longitude_delta).to_degrees();
    let normalized_longitude = (longitude_2 + 180.0).rem_euclid(360.0) - 180.0;

    GeoPoint::new(latitude_2.to_degrees(), normalized_longitude)
}

pub fn distance_and_bearing(
    point_1: GeoPoint,
    point_2: GeoPoint,
) -> Result<(f64, f64), DomainError> {
    let latitude_1 = point_1.latitude().to_radians();
    let latitude_2 = point_2.latitude().to_radians();
    let longitude_delta =
        normalize_radians((point_2.longitude() - point_1.longitude()).to_radians());
    let reduced_latitude_1 = ((1.0 - WGS84_FLATTENING) * latitude_1.tan()).atan();
    let reduced_latitude_2 = ((1.0 - WGS84_FLATTENING) * latitude_2.tan()).atan();
    let (sin_u1, cos_u1) = reduced_latitude_1.sin_cos();
    let (sin_u2, cos_u2) = reduced_latitude_2.sin_cos();

    let mut lambda = longitude_delta;
    let mut converged = false;

    for _ in 0..MAX_ITERATIONS {
        let (sin_lambda, cos_lambda) = lambda.sin_cos();
        let first = cos_u2 * sin_lambda;
        let second = cos_u1 * sin_u2 - sin_u1 * cos_u2 * cos_lambda;
        let sin_sigma = first.hypot(second);
        if sin_sigma == 0.0 {
            return Ok((0.0, 0.0));
        }
        let cos_sigma = sin_u1 * sin_u2 + cos_u1 * cos_u2 * cos_lambda;
        let sigma = sin_sigma.atan2(cos_sigma);
        let sin_alpha = cos_u1 * cos_u2 * sin_lambda / sin_sigma;
        let cos_squared_alpha = 1.0 - sin_alpha.powi(2);
        let cos_two_sigma_midpoint = if cos_squared_alpha <= f64::EPSILON {
            0.0
        } else {
            cos_sigma - 2.0 * sin_u1 * sin_u2 / cos_squared_alpha
        };
        let coefficient_c = WGS84_FLATTENING / 16.0
            * cos_squared_alpha
            * (4.0 + WGS84_FLATTENING * (4.0 - 3.0 * cos_squared_alpha));
        let next_lambda = longitude_delta
            + (1.0 - coefficient_c)
                * WGS84_FLATTENING
                * sin_alpha
                * (sigma
                    + coefficient_c
                        * sin_sigma
                        * (cos_two_sigma_midpoint
                            + coefficient_c
                                * cos_sigma
                                * (-1.0 + 2.0 * cos_two_sigma_midpoint.powi(2))));
        if (next_lambda - lambda).abs() <= CONVERGENCE_TOLERANCE {
            lambda = next_lambda;
            converged = true;
            break;
        }
        lambda = next_lambda;
    }
    if !converged {
        return Err(DomainError::GeodesyDidNotConverge);
    }

    let (sin_lambda, cos_lambda) = lambda.sin_cos();
    let sin_sigma = (cos_u2 * sin_lambda).hypot(cos_u1 * sin_u2 - sin_u1 * cos_u2 * cos_lambda);
    if sin_sigma == 0.0 {
        return Ok((0.0, 0.0));
    }
    let cos_sigma = sin_u1 * sin_u2 + cos_u1 * cos_u2 * cos_lambda;
    let sigma = sin_sigma.atan2(cos_sigma);
    let sin_alpha = cos_u1 * cos_u2 * sin_lambda / sin_sigma;
    let cos_squared_alpha = 1.0 - sin_alpha.powi(2);
    let cos_two_sigma_midpoint = if cos_squared_alpha <= f64::EPSILON {
        0.0
    } else {
        cos_sigma - 2.0 * sin_u1 * sin_u2 / cos_squared_alpha
    };

    let u_squared = cos_squared_alpha
        * (WGS84_SEMI_MAJOR_AXIS_M.powi(2) - WGS84_SEMI_MINOR_AXIS_M.powi(2))
        / WGS84_SEMI_MINOR_AXIS_M.powi(2);
    let coefficient_a = 1.0
        + u_squared / 16_384.0
            * (4_096.0 + u_squared * (-768.0 + u_squared * (320.0 - 175.0 * u_squared)));
    let coefficient_b = u_squared / 1_024.0
        * (256.0 + u_squared * (-128.0 + u_squared * (74.0 - 47.0 * u_squared)));
    let delta_sigma = coefficient_b
        * sin_sigma
        * (cos_two_sigma_midpoint
            + coefficient_b / 4.0
                * (cos_sigma * (-1.0 + 2.0 * cos_two_sigma_midpoint.powi(2))
                    - coefficient_b / 6.0
                        * cos_two_sigma_midpoint
                        * (-3.0 + 4.0 * sin_sigma.powi(2))
                        * (-3.0 + 4.0 * cos_two_sigma_midpoint.powi(2))));
    let distance_m = WGS84_SEMI_MINOR_AXIS_M * coefficient_a * (sigma - delta_sigma);
    let initial_bearing = (cos_u2 * lambda.sin())
        .atan2(cos_u1 * sin_u2 - sin_u1 * cos_u2 * lambda.cos())
        .rem_euclid(2.0 * PI);

    Ok((distance_m, initial_bearing))
}

fn normalize_radians(angle: f64) -> f64 {
    (angle + PI).rem_euclid(2.0 * PI) - PI
}

#[cfg(test)]
mod tests {
    use super::{distance_and_bearing, project_point};
    use crate::types::GeoPoint;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn computes_short_distance_across_antimeridian() {
        let start = GeoPoint::new(0.0, 179.9).unwrap();
        let end = GeoPoint::new(0.0, -179.9).unwrap();
        let (distance, bearing) = distance_and_bearing(start, end).unwrap();

        assert!((22_000.0..23_000.0).contains(&distance));
        assert!((bearing - FRAC_PI_2).abs() < 1e-9);
    }

    #[test]
    fn direct_and_inverse_calculations_agree_within_one_millimeter() {
        let start = GeoPoint::new(37.0, -122.0).unwrap();
        let destination = project_point(start, 1.2, 1_000_000.0).unwrap();
        let (distance, bearing) = distance_and_bearing(start, destination).unwrap();

        assert!((distance - 1_000_000.0).abs() < 0.001);
        assert!((bearing - 1.2).abs() < 1e-9);
    }

    #[test]
    fn inverse_calculation_is_accurate_near_the_pole() {
        let start = GeoPoint::new(89.999, 45.0).unwrap();
        let destination = project_point(start, FRAC_PI_2, 100.0).unwrap();
        let (distance, _) = distance_and_bearing(start, destination).unwrap();

        assert!((distance - 100.0).abs() < 0.001);
    }

    #[test]
    fn rejects_negative_distance_and_handles_coincident_points() {
        let point = GeoPoint::new(0.0, 0.0).unwrap();
        assert!(project_point(point, 0.0, -1.0).is_err());
        assert_eq!(distance_and_bearing(point, point).unwrap(), (0.0, 0.0));

        let west = project_point(point, 3.0 * PI / 2.0, 100.0).unwrap();
        assert!(west.longitude() < 0.0);
    }
}
