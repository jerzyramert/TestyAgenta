use crate::error::DomainError;

const METERS_PER_SECOND_PER_KNOT: f64 = 1852.0 / 3600.0;
const DEGREES_PER_RADIAN: f64 = 180.0 / std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct MetersPerSecond(f64);

impl MetersPerSecond {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Knots(f64);

impl Knots {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> f64 {
        self.0
    }
}

impl From<Knots> for MetersPerSecond {
    fn from(knots: Knots) -> Self {
        Self(knots.0 * METERS_PER_SECOND_PER_KNOT)
    }
}

impl From<MetersPerSecond> for Knots {
    fn from(speed: MetersPerSecond) -> Self {
        Self(speed.0 / METERS_PER_SECOND_PER_KNOT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Radians(f64);

impl Radians {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> f64 {
        self.0
    }

    pub fn normalized_bearing(self) -> Self {
        Self(self.0.rem_euclid(2.0 * std::f64::consts::PI))
    }

    pub fn normalized_relative(self) -> Self {
        Self(
            (self.0 + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI)
                - std::f64::consts::PI,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Degrees(f64);

impl Degrees {
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> f64 {
        self.0
    }

    pub fn normalized_bearing(self) -> Self {
        Self(self.0.rem_euclid(360.0))
    }
}

impl From<Degrees> for Radians {
    fn from(degrees: Degrees) -> Self {
        Self(degrees.0 / DEGREES_PER_RADIAN)
    }
}

impl From<Radians> for Degrees {
    fn from(radians: Radians) -> Self {
        Self(radians.0 * DEGREES_PER_RADIAN)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    lat: f64,
    lon: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherSample {
    pub wind_u: f64,
    pub wind_v: f64,
    pub gust: f64,
    pub current_u: f64,
    pub current_v: f64,
    pub wave_height: f64,
    pub wave_period: f64,
    pub wave_dir: f64,
    pub is_climatology: bool,
}

impl GeoPoint {
    pub fn new(lat: f64, lon: f64) -> Result<Self, DomainError> {
        if !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
        {
            return Err(DomainError::InvalidCoordinate {
                latitude: lat,
                longitude: lon,
            });
        }

        Ok(Self { lat, lon })
    }

    pub const fn latitude(self) -> f64 {
        self.lat
    }

    pub const fn longitude(self) -> f64 {
        self.lon
    }
}

#[cfg(test)]
mod tests {
    use super::{Degrees, GeoPoint, Knots, MetersPerSecond, Radians};
    use crate::error::DomainError;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn converts_knots_to_meters_per_second_and_back() {
        let speed = MetersPerSecond::from(Knots::new(10.0));

        assert!((speed.value() - 5.144_444_444_444_445).abs() < 1e-12);
        assert!((Knots::from(speed).value() - 10.0).abs() < 1e-12);
    }

    #[test]
    fn converts_degrees_to_radians_and_normalizes_angles() {
        let angle = Radians::from(Degrees::new(90.0));
        assert!((angle.value() - FRAC_PI_2).abs() < 1e-12);

        let bearing = Radians::new(-FRAC_PI_2).normalized_bearing();
        assert!((bearing.value() - 3.0 * FRAC_PI_2).abs() < 1e-12);

        let relative = Radians::new(3.0 * PI).normalized_relative();
        assert!((relative.value() + PI).abs() < 1e-12);
    }

    #[test]
    fn validates_geographic_coordinates() {
        assert!(GeoPoint::new(90.0, -180.0).is_ok());
        assert_eq!(
            GeoPoint::new(91.0, 0.0),
            Err(DomainError::InvalidCoordinate {
                latitude: 91.0,
                longitude: 0.0,
            })
        );
    }
}
