use tokio_stream::{Stream, StreamExt};

use super::{CalculateRouteInput, CoordinateInput};
use crate::error::DomainError;
use crate::spatial::geodesy::{distance_and_bearing, project_point};
use crate::types::GeoPoint;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nmea2000Frame {
    pub pgn: u32,
    pub payload: Vec<u8>,
    pub received_at_unix: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NmeaMeasurement {
    CourseAndSpeed {
        course_rad: Option<f64>,
        speed_mps: Option<f64>,
    },
    Wind {
        speed_mps: Option<f64>,
        angle_rad: Option<f64>,
        reference: u8,
    },
    Heading {
        heading_rad: Option<f64>,
        reference: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NmeaRerouteEvent {
    pub current_position: GeoPoint,
    pub target: GeoPoint,
    pub cross_track_error_m: f64,
    pub observed_at_unix: i64,
    pub course_rad: Option<f64>,
}

impl NmeaRerouteEvent {
    pub fn updated_route_input(&self, original: &CalculateRouteInput) -> CalculateRouteInput {
        let mut input = original.clone();
        input.start = CoordinateInput {
            latitude: self.current_position.latitude(),
            longitude: self.current_position.longitude(),
        };
        input.target = CoordinateInput {
            latitude: self.target.latitude(),
            longitude: self.target.longitude(),
        };
        input.departure_time_unix = self.observed_at_unix;
        if let Some(course_rad) = self.course_rad {
            input.initial_heading_degrees = course_rad.to_degrees().rem_euclid(360.0);
        }
        input
    }
}

pub struct NmeaRerouteMonitor {
    route: Vec<GeoPoint>,
    current_position: GeoPoint,
    last_update_unix: i64,
    course_rad: Option<f64>,
    speed_mps: Option<f64>,
    maximum_xte_m: f64,
    over_threshold: bool,
}

impl NmeaRerouteMonitor {
    pub fn new(
        route: Vec<GeoPoint>,
        initial_position: GeoPoint,
        initial_time_unix: i64,
        maximum_xte_m: f64,
    ) -> Result<Self, DomainError> {
        if route.len() < 2 {
            return Err(invalid_nmea("route must contain at least two points"));
        }
        if !maximum_xte_m.is_finite() || maximum_xte_m <= 0.0 {
            return Err(invalid_nmea(
                "maximum cross-track error must be finite and positive",
            ));
        }

        Ok(Self {
            route,
            current_position: initial_position,
            last_update_unix: initial_time_unix,
            course_rad: None,
            speed_mps: None,
            maximum_xte_m,
            over_threshold: false,
        })
    }

    pub fn ingest(
        &mut self,
        frame: &Nmea2000Frame,
    ) -> Result<Option<NmeaRerouteEvent>, DomainError> {
        if frame.received_at_unix < self.last_update_unix {
            return Err(invalid_nmea("frame timestamps must not move backwards"));
        }

        if let (Some(course), Some(speed)) = (self.course_rad, self.speed_mps) {
            let elapsed_seconds = frame
                .received_at_unix
                .checked_sub(self.last_update_unix)
                .ok_or_else(|| invalid_nmea("frame time interval is out of range"))?
                as f64;
            self.current_position =
                project_point(self.current_position, course, speed * elapsed_seconds)?;
        }
        self.last_update_unix = frame.received_at_unix;

        if let NmeaMeasurement::CourseAndSpeed {
            course_rad,
            speed_mps,
        } = decode_pgn(frame.pgn, &frame.payload)?
        {
            self.course_rad = course_rad;
            self.speed_mps = speed_mps;
        }

        let xte_m = route_xte_m(&self.route, self.current_position)?;
        if xte_m <= self.maximum_xte_m {
            self.over_threshold = false;
            return Ok(None);
        }
        if self.over_threshold {
            return Ok(None);
        }

        self.over_threshold = true;
        let Some(&target) = self.route.last() else {
            return Err(invalid_nmea("route must contain a target point"));
        };
        Ok(Some(NmeaRerouteEvent {
            current_position: self.current_position,
            target,
            cross_track_error_m: xte_m,
            observed_at_unix: frame.received_at_unix,
            course_rad: self.course_rad,
        }))
    }

    pub async fn listen<S, F>(
        &mut self,
        mut frames: S,
        mut on_reroute: F,
    ) -> Result<(), DomainError>
    where
        S: Stream<Item = Nmea2000Frame> + Unpin,
        F: FnMut(NmeaRerouteEvent),
    {
        while let Some(frame) = frames.next().await {
            if let Some(event) = self.ingest(&frame)? {
                on_reroute(event);
            }
        }
        Ok(())
    }
}

pub fn decode_pgn(pgn: u32, payload: &[u8]) -> Result<NmeaMeasurement, DomainError> {
    if payload.len() != 8 {
        return Err(invalid_nmea("supported PGNs require an 8-byte payload"));
    }

    match pgn {
        129_026 => {
            let reference = payload[1] & 0x03;
            if reference != 0 {
                return Err(invalid_nmea(
                    "PGN 129026 must provide course over ground referenced to true north",
                ));
            }
            let course = little_endian_u16(payload, 2);
            let speed = little_endian_u16(payload, 4);
            let course_rad = (course != u16::MAX).then_some(course as f64 * 0.0001);
            if course_rad.is_some_and(|value| value > std::f64::consts::TAU) {
                return Err(invalid_nmea("PGN 129026 course is outside its valid range"));
            }
            Ok(NmeaMeasurement::CourseAndSpeed {
                course_rad,
                speed_mps: (speed != u16::MAX).then_some(speed as f64 * 0.01),
            })
        }
        130_306 => {
            let speed = little_endian_u16(payload, 1);
            let angle = little_endian_u16(payload, 3);
            let angle_rad = (angle != u16::MAX).then_some(angle as f64 * 0.0001);
            if angle_rad.is_some_and(|value| value > std::f64::consts::TAU) {
                return Err(invalid_nmea("PGN 130306 angle is outside its valid range"));
            }
            Ok(NmeaMeasurement::Wind {
                speed_mps: (speed != u16::MAX).then_some(speed as f64 * 0.01),
                angle_rad,
                reference: payload[5] & 0x07,
            })
        }
        127_250 => {
            let heading = little_endian_u16(payload, 1);
            let heading_rad = (heading != u16::MAX).then_some(heading as f64 * 0.0001);
            if heading_rad.is_some_and(|value| value > std::f64::consts::TAU) {
                return Err(invalid_nmea(
                    "PGN 127250 heading is outside its valid range",
                ));
            }
            Ok(NmeaMeasurement::Heading {
                heading_rad,
                reference: payload[7] & 0x03,
            })
        }
        _ => Err(DomainError::UnsupportedNmeaPgn(pgn)),
    }
}

fn route_xte_m(route: &[GeoPoint], position: GeoPoint) -> Result<f64, DomainError> {
    route
        .windows(2)
        .map(|segment| cross_track_to_segment_m(segment[0], segment[1], position))
        .try_fold(
            f64::INFINITY,
            |minimum, distance| Ok(minimum.min(distance?)),
        )
}

fn cross_track_to_segment_m(
    start: GeoPoint,
    end: GeoPoint,
    point: GeoPoint,
) -> Result<f64, DomainError> {
    let (segment_length_m, segment_bearing_rad) = distance_and_bearing(start, end)?;
    let (distance_to_point_m, bearing_to_point_rad) = distance_and_bearing(start, point)?;
    if segment_length_m == 0.0 || distance_to_point_m == 0.0 {
        return Ok(distance_to_point_m);
    }

    let bearing_delta = normalize_radians(bearing_to_point_rad - segment_bearing_rad);
    let angular_distance = distance_to_point_m / EARTH_RADIUS_M;
    let along_track_m = (angular_distance.sin() * bearing_delta.cos())
        .atan2(angular_distance.cos())
        * EARTH_RADIUS_M;

    if along_track_m < 0.0 {
        return Ok(distance_to_point_m);
    }
    if along_track_m > segment_length_m {
        return Ok(distance_and_bearing(end, point)?.0);
    }

    let cross_track_angle = (angular_distance.sin() * bearing_delta.sin())
        .clamp(-1.0, 1.0)
        .asin();
    Ok(cross_track_angle.abs() * EARTH_RADIUS_M)
}

fn normalize_radians(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

fn little_endian_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn invalid_nmea(message: &str) -> DomainError {
    DomainError::InvalidNmeaFrame(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{decode_pgn, Nmea2000Frame, NmeaMeasurement, NmeaRerouteMonitor};
    use crate::api::{CalculateRouteInput, CoordinateInput};
    use crate::error::DomainError;
    use crate::spatial::geodesy::project_point;
    use crate::types::GeoPoint;

    fn point(lat: f64, lon: f64) -> GeoPoint {
        GeoPoint::new(lat, lon).unwrap()
    }

    fn cog_sog_frame(course_rad: f64, speed_mps: f64, timestamp: i64) -> Nmea2000Frame {
        let course = (course_rad / 0.0001).round() as u16;
        let speed = (speed_mps / 0.01).round() as u16;
        let mut payload = vec![0_u8; 8];
        payload[2..4].copy_from_slice(&course.to_le_bytes());
        payload[4..6].copy_from_slice(&speed.to_le_bytes());
        Nmea2000Frame {
            pgn: 129_026,
            payload,
            received_at_unix: timestamp,
        }
    }

    #[test]
    fn decodes_cog_sog_and_non_navigation_pgns() {
        assert_eq!(
            decode_pgn(129_026, &cog_sog_frame(1.2, 4.5, 0).payload).unwrap(),
            NmeaMeasurement::CourseAndSpeed {
                course_rad: Some(1.2),
                speed_mps: Some(4.5),
            }
        );
        assert!(matches!(
            decode_pgn(130_306, &[0; 8]).unwrap(),
            NmeaMeasurement::Wind { .. }
        ));
        assert!(matches!(
            decode_pgn(127_250, &[0; 8]).unwrap(),
            NmeaMeasurement::Heading { .. }
        ));
    }

    #[test]
    fn requires_true_course_and_rejects_malformed_or_unsupported_frames() {
        let mut magnetic = [0_u8; 8];
        magnetic[1] = 1;
        assert!(matches!(
            decode_pgn(129_026, &magnetic),
            Err(DomainError::InvalidNmeaFrame(_))
        ));
        assert!(matches!(
            decode_pgn(129_026, &[0; 7]),
            Err(DomainError::InvalidNmeaFrame(_))
        ));
        assert_eq!(
            decode_pgn(1, &[0; 8]),
            Err(DomainError::UnsupportedNmeaPgn(1))
        );
    }

    #[test]
    fn reroute_triggers_once_when_dead_reckoning_exceeds_xte_threshold() {
        let start = point(0.0, 0.0);
        let target = point(0.0, 0.1);
        let mut monitor = NmeaRerouteMonitor::new(vec![start, target], start, 0, 100.0).unwrap();
        let northbound = cog_sog_frame(0.0, 10.0, 1);
        assert_eq!(monitor.ingest(&northbound).unwrap(), None);

        let after_one_hour = project_point(start, 0.0, 36_000.0).unwrap();
        let northbound_after_hour = Nmea2000Frame {
            received_at_unix: 3_601,
            ..northbound.clone()
        };
        let event = monitor.ingest(&northbound_after_hour).unwrap().unwrap();

        assert_eq!(event.target, target);
        let updated = event.updated_route_input(&CalculateRouteInput {
            vessel_id: "boat".to_owned(),
            start: CoordinateInput {
                latitude: start.latitude(),
                longitude: start.longitude(),
            },
            target: CoordinateInput {
                latitude: target.latitude(),
                longitude: target.longitude(),
            },
            departure_time_unix: 0,
            initial_heading_degrees: 0.0,
            time_step_seconds: None,
            heading_step_degrees: None,
            max_tws_knots: None,
            max_wave_height_m: None,
            max_steps: None,
        });
        assert_eq!(updated.target.latitude, target.latitude());
        assert_eq!(updated.target.longitude, target.longitude());
        assert_eq!(updated.departure_time_unix, 3_601);
        assert!((updated.initial_heading_degrees - 0.0).abs() < 1e-12);
        assert!((event.current_position.latitude() - after_one_hour.latitude()).abs() < 1e-8);
        assert!(event.cross_track_error_m > 100.0);
        assert!(monitor.ingest(&northbound_after_hour).unwrap().is_none());
    }

    #[test]
    fn reroute_rearms_after_position_returns_within_threshold() {
        let start = point(0.0, 0.0);
        let target = point(0.0, 0.1);
        let north = point(0.01, 0.0);
        let mut monitor = NmeaRerouteMonitor::new(vec![start, target], north, 10, 100.0).unwrap();
        let southbound = cog_sog_frame(std::f64::consts::PI, 10.0, 10);
        assert!(monitor.ingest(&southbound).unwrap().is_some());

        let nearing_route = Nmea2000Frame {
            pgn: 130_306,
            payload: vec![0; 8],
            received_at_unix: 121,
        };
        assert!(monitor.ingest(&nearing_route).unwrap().is_none());
        let departed_again = Nmea2000Frame {
            pgn: 130_306,
            payload: vec![0; 8],
            received_at_unix: 322,
        };
        assert!(monitor.ingest(&departed_again).unwrap().is_some());
    }

    #[test]
    fn validates_route_and_monotonic_frame_time() {
        assert!(NmeaRerouteMonitor::new(vec![point(0.0, 0.0)], point(0.0, 0.0), 0, 10.0).is_err());
        let mut monitor = NmeaRerouteMonitor::new(
            vec![point(0.0, 0.0), point(0.0, 1.0)],
            point(0.0, 0.0),
            10,
            100.0,
        )
        .unwrap();
        let frame = Nmea2000Frame {
            pgn: 130_306,
            payload: vec![0; 8],
            received_at_unix: 9,
        };
        assert!(matches!(
            monitor.ingest(&frame),
            Err(DomainError::InvalidNmeaFrame(_))
        ));
    }
}
