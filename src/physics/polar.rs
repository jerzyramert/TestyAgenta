use std::collections::HashMap;
use std::f64::consts::PI;
use std::io::Cursor;

use redis::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::DomainError;

const KNOT_TO_MPS: f64 = 1_852.0 / 3_600.0;
const INVALID_SPEED_SENTINEL: f64 = 99.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvFormat {
    Normalized,
    Matrix,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VmgValue {
    pub angle_rad: f64,
    pub speed_mps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VmgSample {
    pub tws_mps: f64,
    pub beat: Option<VmgValue>,
    pub run: Option<VmgValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SailPolar {
    pub sail_id: String,
    pub description: Option<String>,
    pub speeds_mps: Vec<Vec<Option<f64>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Polar {
    tws_mps: Vec<f64>,
    twa_rad: Vec<f64>,
    sails: Vec<SailPolar>,
    vmg: Vec<VmgSample>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarPerformance<'a> {
    pub sail_id: &'a str,
    pub speed_mps: f64,
}

#[derive(Debug)]
struct SailInput {
    sail_id: String,
    description: Option<String>,
    speeds_mps: Vec<Vec<Option<f64>>>,
}

type CsvSailMatrix = (String, Vec<Vec<Option<f64>>>);

impl Polar {
    pub fn parse_pol(input: &str) -> Result<Self, DomainError> {
        let data_lines: Vec<&str> = input
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('#'))
            .collect();
        let header = data_lines
            .first()
            .ok_or_else(|| invalid_polar("missing .pol TWS header"))?;
        let tws_knots = parse_axis_header(header, "TWS")?;
        if tws_knots.len() < 2 {
            return Err(invalid_polar(
                ".pol TWS axis must contain at least two values",
            ));
        }

        let mut twa_degrees = Vec::new();
        let mut speeds_mps = Vec::new();
        for (line_index, line) in data_lines.iter().enumerate().skip(1) {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() != tws_knots.len() + 1 {
                return Err(invalid_polar(format!(
                    ".pol row {} has {} speed cells; expected {}",
                    line_index + 1,
                    tokens.len().saturating_sub(1),
                    tws_knots.len()
                )));
            }
            let twa = parse_required_number(tokens[0], "TWA")?;
            twa_degrees.push(twa);
            speeds_mps.push(
                tokens[1..]
                    .iter()
                    .map(|token| parse_speed_cell(token))
                    .map(|speed| speed.map(|value| value * KNOT_TO_MPS))
                    .collect(),
            );
        }

        Self::from_si(
            tws_knots
                .into_iter()
                .map(|value| value * KNOT_TO_MPS)
                .collect(),
            twa_degrees,
            vec![SailInput {
                sail_id: "default".to_owned(),
                description: None,
                speeds_mps,
            }],
        )
    }

    pub fn parse_orc_json(input: &str) -> Result<Self, DomainError> {
        let root: Value = serde_json::from_str(input)
            .map_err(|error| invalid_polar(format!("invalid ORC JSON: {error}")))?;
        let metadata = root
            .get("metadata")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_polar("ORC JSON is missing metadata"))?;
        let tws_unit = parse_speed_unit(
            metadata
                .get("unitTWS")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_polar("ORC metadata is missing unitTWS"))?,
        )?;
        let boat_speed_unit = parse_speed_unit(
            metadata
                .get("unitBS")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_polar("ORC metadata is missing unitBS"))?,
        )?;
        if metadata.get("boatName").and_then(Value::as_str).is_none() {
            return Err(invalid_polar("ORC metadata is missing boatName"));
        }

        let tws = parse_json_axis(root.get("tws_grid"), "tws_grid")?;
        let twa = parse_json_axis(root.get("twa_grid"), "twa_grid")?;
        let configurations = root
            .get("configurations")
            .and_then(Value::as_array)
            .filter(|configurations| !configurations.is_empty())
            .ok_or_else(|| invalid_polar("ORC JSON needs at least one configuration"))?;

        let mut sails = Vec::with_capacity(configurations.len());
        for configuration in configurations {
            let sail_id = configuration
                .get("sail_id")
                .and_then(Value::as_str)
                .filter(|sail_id| !sail_id.trim().is_empty())
                .ok_or_else(|| invalid_polar("ORC configuration is missing sail_id"))?
                .to_owned();
            let description = configuration
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let rows = configuration
                .get("matrix")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid_polar(format!("sail {sail_id} is missing matrix")))?;
            if rows.len() != twa.len() {
                return Err(invalid_polar(format!(
                    "sail {sail_id} matrix has {} TWA rows; expected {}",
                    rows.len(),
                    twa.len()
                )));
            }
            let mut speeds_mps = Vec::with_capacity(rows.len());
            for row in rows {
                let cells = row.as_array().ok_or_else(|| {
                    invalid_polar(format!("sail {sail_id} matrix row is not an array"))
                })?;
                if cells.len() != tws.len() {
                    return Err(invalid_polar(format!(
                        "sail {sail_id} matrix row has {} TWS cells; expected {}",
                        cells.len(),
                        tws.len()
                    )));
                }
                speeds_mps.push(
                    cells
                        .iter()
                        .map(|cell| parse_json_speed_cell(cell, boat_speed_unit))
                        .collect(),
                );
            }
            sails.push(SailInput {
                sail_id,
                description,
                speeds_mps,
            });
        }

        let tws_mps = tws.iter().map(|value| *value * tws_unit).collect();
        Self::from_si(tws_mps, twa, sails)
    }

    pub fn parse_csv(input: &str, format: CsvFormat) -> Result<Self, DomainError> {
        match format {
            CsvFormat::Normalized => Self::parse_normalized_csv(input),
            CsvFormat::Matrix => Self::parse_matrix_csv(input),
        }
    }

    pub fn tws_axis_mps(&self) -> &[f64] {
        &self.tws_mps
    }

    pub fn twa_axis_rad(&self) -> &[f64] {
        &self.twa_rad
    }

    pub fn sails(&self) -> &[SailPolar] {
        &self.sails
    }

    pub fn vmg_table(&self) -> &[VmgSample] {
        &self.vmg
    }

    pub fn maximum_speed_mps(&self) -> f64 {
        self.sails
            .iter()
            .flat_map(|sail| sail.speeds_mps.iter().flatten().flatten())
            .copied()
            .fold(0.0, f64::max)
    }

    pub fn crossover_matrix(&self) -> Vec<Vec<Option<&str>>> {
        self.twa_rad
            .iter()
            .enumerate()
            .map(|(twa_index, _)| {
                self.tws_mps
                    .iter()
                    .enumerate()
                    .map(|(tws_index, _)| {
                        self.sails
                            .iter()
                            .filter_map(|sail| {
                                sail.speeds_mps[twa_index][tws_index]
                                    .map(|speed| (sail.sail_id.as_str(), speed))
                            })
                            .max_by(|left, right| left.1.total_cmp(&right.1))
                            .map(|(sail_id, _)| sail_id)
                    })
                    .collect()
            })
            .collect()
    }

    pub fn speed_for_sail(
        &self,
        sail_id: &str,
        twa_rad: f64,
        tws_mps: f64,
    ) -> Result<Option<f64>, DomainError> {
        let sail = self
            .sails
            .iter()
            .find(|sail| sail.sail_id == sail_id)
            .ok_or_else(|| invalid_polar(format!("unknown sail_id {sail_id}")))?;
        self.interpolate_speed(sail, twa_rad, tws_mps)
    }

    pub fn best_speed(
        &self,
        twa_rad: f64,
        tws_mps: f64,
    ) -> Result<Option<PolarPerformance<'_>>, DomainError> {
        let mut best: Option<PolarPerformance<'_>> = None;
        for sail in &self.sails {
            if let Some(speed_mps) = self.interpolate_speed(sail, twa_rad, tws_mps)? {
                if best.map_or(true, |current| speed_mps > current.speed_mps) {
                    best = Some(PolarPerformance {
                        sail_id: &sail.sail_id,
                        speed_mps,
                    });
                }
            }
        }
        Ok(best)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_axis(&self.tws_mps, "TWS")?;
        validate_axis(&self.twa_rad, "TWA")?;
        if self.tws_mps[0] <= 0.0 {
            return Err(invalid_polar("cached TWS values must be positive"));
        }
        if self.twa_rad.first().map_or(true, |angle| *angle < 0.0)
            || self.twa_rad.last().map_or(true, |angle| *angle > PI)
        {
            return Err(invalid_polar("TWA axis must be within 0..180 degrees"));
        }
        if self.sails.is_empty() || self.vmg.len() != self.tws_mps.len() {
            return Err(invalid_polar("cached polar has inconsistent dimensions"));
        }
        let mut ids = Vec::with_capacity(self.sails.len());
        for sail in &self.sails {
            if sail.sail_id.trim().is_empty()
                || ids.iter().any(|id: &&str| *id == sail.sail_id)
                || sail.speeds_mps.len() != self.twa_rad.len()
                || sail
                    .speeds_mps
                    .iter()
                    .any(|row| row.len() != self.tws_mps.len())
            {
                return Err(invalid_polar("cached sail matrix is invalid"));
            }
            ids.push(sail.sail_id.as_str());
            if sail
                .speeds_mps
                .iter()
                .flatten()
                .flatten()
                .any(|speed| !speed.is_finite() || *speed < 0.0)
            {
                return Err(invalid_polar("cached boat speed is invalid"));
            }
        }
        for (tws, sample) in self.tws_mps.iter().zip(&self.vmg) {
            if sample.tws_mps != *tws || sample.beat.is_none() || sample.run.is_none() {
                return Err(invalid_polar("cached VMG table is incomplete"));
            }
            for value in [sample.beat.as_ref(), sample.run.as_ref()]
                .into_iter()
                .flatten()
            {
                if !value.angle_rad.is_finite()
                    || !(0.0..=PI).contains(&value.angle_rad)
                    || !value.speed_mps.is_finite()
                    || value.speed_mps < 0.0
                {
                    return Err(invalid_polar("cached VMG value is invalid"));
                }
            }
        }
        Ok(())
    }

    pub fn cache_key(vessel_id: &str) -> Result<String, DomainError> {
        if vessel_id.trim().is_empty() {
            return Err(invalid_polar("vessel_id must not be empty"));
        }
        Ok(format!("vpp:{vessel_id}"))
    }

    pub fn store_in_redis(
        connection: &mut Connection,
        vessel_id: &str,
        polar: &Self,
    ) -> Result<(), DomainError> {
        polar.validate()?;
        let key = Self::cache_key(vessel_id)?;
        let payload = serde_json::to_string(polar)
            .map_err(|error| DomainError::PolarCache(error.to_string()))?;
        redis::cmd("SET")
            .arg(key)
            .arg(payload)
            .query::<()>(connection)
            .map_err(|error| DomainError::PolarCache(error.to_string()))
    }

    pub fn load_from_redis(
        connection: &mut Connection,
        vessel_id: &str,
    ) -> Result<Option<Self>, DomainError> {
        let key = Self::cache_key(vessel_id)?;
        let payload: Option<String> = redis::cmd("GET")
            .arg(key)
            .query(connection)
            .map_err(|error| DomainError::PolarCache(error.to_string()))?;
        payload
            .map(|payload| {
                let polar: Self = serde_json::from_str(&payload)
                    .map_err(|error| DomainError::PolarCache(error.to_string()))?;
                polar.validate()?;
                Ok(polar)
            })
            .transpose()
    }

    fn parse_normalized_csv(input: &str) -> Result<Self, DomainError> {
        let mut reader = csv::ReaderBuilder::new()
            .trim(csv::Trim::All)
            .from_reader(Cursor::new(input));
        let headers = reader
            .headers()
            .map_err(|error| invalid_polar(format!("invalid CSV header: {error}")))?;
        if headers.iter().collect::<Vec<_>>() != ["TWA", "TWS", "BS", "SailID"] {
            return Err(invalid_polar(
                "normalized CSV header must be TWA,TWS,BS,SailID",
            ));
        }

        let mut records = Vec::new();
        let mut twa_axis = Vec::new();
        let mut tws_axis = Vec::new();
        let mut sail_ids = Vec::new();
        for record in reader.records() {
            let record =
                record.map_err(|error| invalid_polar(format!("invalid CSV row: {error}")))?;
            let twa = parse_required_number(
                record
                    .get(0)
                    .ok_or_else(|| invalid_polar("CSV row is missing TWA"))?,
                "CSV TWA",
            )?;
            let tws = parse_required_number(
                record
                    .get(1)
                    .ok_or_else(|| invalid_polar("CSV row is missing TWS"))?,
                "CSV TWS",
            )?;
            let speed = record
                .get(2)
                .ok_or_else(|| invalid_polar("CSV row is missing BS"))?;
            let sail_id = record
                .get(3)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid_polar("CSV SailID must not be empty"))?
                .to_owned();
            twa_axis.push(twa);
            tws_axis.push(tws);
            if !sail_ids.contains(&sail_id) {
                sail_ids.push(sail_id.clone());
            }
            records.push((twa, tws, parse_speed_cell(speed), sail_id));
        }
        sort_dedup_axis(&mut twa_axis);
        sort_dedup_axis(&mut tws_axis);
        let sails = assemble_csv_sails(&twa_axis, &tws_axis, &sail_ids, records)?;
        Self::from_si(
            tws_axis
                .into_iter()
                .map(|value| value * KNOT_TO_MPS)
                .collect(),
            twa_axis,
            sails
                .into_iter()
                .map(|(sail_id, speeds_mps)| SailInput {
                    sail_id,
                    description: None,
                    speeds_mps,
                })
                .collect(),
        )
    }

    fn parse_matrix_csv(input: &str) -> Result<Self, DomainError> {
        let lines: Vec<&str> = input
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('#'))
            .collect();
        let header = lines
            .first()
            .ok_or_else(|| invalid_polar("missing matrix CSV TWS header"))?;
        let tws_knots = parse_axis_header(header, "TWS")?;
        if tws_knots.len() < 2 {
            return Err(invalid_polar(
                "matrix CSV TWS axis needs at least two values",
            ));
        }
        let mut twa_degrees = Vec::new();
        let mut speeds_mps = Vec::new();
        for (line_index, line) in lines.iter().enumerate().skip(1) {
            let mut tokens = line.split(',').map(str::trim);
            let twa = tokens
                .next()
                .ok_or_else(|| invalid_polar("matrix CSV row is empty"))?;
            let cells: Vec<&str> = tokens.collect();
            if cells.len() != tws_knots.len() {
                return Err(invalid_polar(format!(
                    "matrix CSV row {} has {} cells; expected {}",
                    line_index + 1,
                    cells.len(),
                    tws_knots.len()
                )));
            }
            twa_degrees.push(parse_required_number(twa, "matrix CSV TWA")?);
            speeds_mps.push(
                cells
                    .into_iter()
                    .map(parse_speed_cell)
                    .map(|speed| speed.map(|value| value * KNOT_TO_MPS))
                    .collect(),
            );
        }
        Self::from_si(
            tws_knots
                .into_iter()
                .map(|value| value * KNOT_TO_MPS)
                .collect(),
            twa_degrees,
            vec![SailInput {
                sail_id: "default".to_owned(),
                description: None,
                speeds_mps,
            }],
        )
    }

    fn from_si(
        tws_mps: Vec<f64>,
        twa_degrees: Vec<f64>,
        sails: Vec<SailInput>,
    ) -> Result<Self, DomainError> {
        validate_axis(&tws_mps, "TWS")?;
        validate_axis(&twa_degrees, "TWA")?;
        if tws_mps[0] <= 0.0 {
            return Err(invalid_polar("TWS values must be positive"));
        }
        if twa_degrees[0] < 0.0 || twa_degrees.last().map_or(true, |last| *last > 180.0) {
            return Err(invalid_polar("TWA values must be within 0..180 degrees"));
        }
        if sails.is_empty() {
            return Err(invalid_polar("polar needs at least one sail configuration"));
        }

        let twa_rad: Vec<f64> = twa_degrees.iter().map(|value| value.to_radians()).collect();
        let mut sail_ids = Vec::with_capacity(sails.len());
        let mut converted_sails = Vec::with_capacity(sails.len());
        for sail in sails {
            if sail.sail_id.trim().is_empty() || sail_ids.contains(&sail.sail_id) {
                return Err(invalid_polar("sail_id values must be non-empty and unique"));
            }
            if sail.speeds_mps.len() != twa_degrees.len()
                || sail.speeds_mps.iter().any(|row| row.len() != tws_mps.len())
            {
                return Err(invalid_polar(format!(
                    "sail {} matrix dimensions do not match the axes",
                    sail.sail_id
                )));
            }
            sail_ids.push(sail.sail_id.clone());
            if sail
                .speeds_mps
                .iter()
                .flatten()
                .flatten()
                .any(|speed| !speed.is_finite() || *speed < 0.0)
            {
                return Err(invalid_polar(format!(
                    "sail {} contains an invalid speed",
                    sail.sail_id
                )));
            }
            converted_sails.push(SailPolar {
                sail_id: sail.sail_id,
                description: sail.description,
                speeds_mps: sail.speeds_mps,
            });
        }

        let vmg = compute_vmg(&tws_mps, &twa_rad, &converted_sails);
        let polar = Self {
            tws_mps,
            twa_rad,
            sails: converted_sails,
            vmg,
        };
        polar.validate()?;
        if polar
            .vmg
            .iter()
            .any(|sample| sample.beat.is_none() || sample.run.is_none())
        {
            return Err(invalid_polar(
                "each TWS column needs reachable upwind and downwind VMG data",
            ));
        }
        Ok(polar)
    }

    fn interpolate_speed(
        &self,
        sail: &SailPolar,
        twa_rad: f64,
        tws_mps: f64,
    ) -> Result<Option<f64>, DomainError> {
        if !twa_rad.is_finite() || !tws_mps.is_finite() || tws_mps < 0.0 {
            return Err(invalid_polar(
                "TWA and TWS queries must be finite and TWS non-negative",
            ));
        }
        let angle = normalize_twa(twa_rad);
        let (twa_lower, twa_upper, twa_fraction) = bracket(&self.twa_rad, angle);
        let below_minimum = tws_mps < self.tws_mps[0];
        let (tws_lower, tws_upper, tws_fraction) = if below_minimum {
            (0, 0, 0.0)
        } else {
            bracket(&self.tws_mps, tws_mps)
        };
        if tws_mps > self.tws_mps[self.tws_mps.len() - 1] {
            return Err(invalid_polar("TWS query exceeds the polar grid"));
        }
        let Some(mut speed) = bilinear(
            &sail.speeds_mps,
            twa_lower,
            twa_upper,
            twa_fraction,
            tws_lower,
            tws_upper,
            tws_fraction,
        ) else {
            return Ok(None);
        };
        if below_minimum {
            speed *= (tws_mps / self.tws_mps[0]).powi(2);
        }

        let (beat_angle, run_angle) = self.vmg_angles_at(tws_mps)?;
        let factor = no_sail_taper(angle, beat_angle, run_angle);
        Ok(Some(speed * factor))
    }

    fn vmg_angles_at(&self, tws_mps: f64) -> Result<(f64, f64), DomainError> {
        if tws_mps > self.tws_mps[self.tws_mps.len() - 1] {
            return Err(invalid_polar("TWS query exceeds the polar grid"));
        }
        let (lower, upper, fraction) = if tws_mps < self.tws_mps[0] {
            (0, 0, 0.0)
        } else {
            bracket(&self.tws_mps, tws_mps)
        };
        let lower_sample = &self.vmg[lower];
        let upper_sample = &self.vmg[upper];
        let beat_lower = lower_sample
            .beat
            .as_ref()
            .ok_or_else(|| invalid_polar("upwind VMG is unavailable"))?;
        let beat_upper = upper_sample
            .beat
            .as_ref()
            .ok_or_else(|| invalid_polar("upwind VMG is unavailable"))?;
        let run_lower = lower_sample
            .run
            .as_ref()
            .ok_or_else(|| invalid_polar("downwind VMG is unavailable"))?;
        let run_upper = upper_sample
            .run
            .as_ref()
            .ok_or_else(|| invalid_polar("downwind VMG is unavailable"))?;
        Ok((
            lerp(beat_lower.angle_rad, beat_upper.angle_rad, fraction),
            lerp(run_lower.angle_rad, run_upper.angle_rad, fraction),
        ))
    }
}

pub struct PolarCache;

impl PolarCache {
    pub fn set(
        connection: &mut Connection,
        vessel_id: &str,
        polar: &Polar,
    ) -> Result<(), DomainError> {
        Polar::store_in_redis(connection, vessel_id, polar)
    }

    pub fn get(connection: &mut Connection, vessel_id: &str) -> Result<Option<Polar>, DomainError> {
        Polar::load_from_redis(connection, vessel_id)
    }
}

fn compute_vmg(tws_mps: &[f64], twa_rad: &[f64], sails: &[SailPolar]) -> Vec<VmgSample> {
    tws_mps
        .iter()
        .enumerate()
        .map(|(tws_index, tws)| {
            let best_at_angle: Vec<Option<f64>> = (0..twa_rad.len())
                .map(|twa_index| {
                    sails
                        .iter()
                        .filter_map(|sail| sail.speeds_mps[twa_index][tws_index])
                        .max_by(f64::total_cmp)
                })
                .collect();
            let best_vmg = |upwind: bool| {
                twa_rad
                    .iter()
                    .enumerate()
                    .filter(|(_, angle)| {
                        if upwind {
                            **angle <= PI / 2.0
                        } else {
                            **angle >= PI / 2.0
                        }
                    })
                    .filter_map(|(index, angle)| {
                        let speed = best_at_angle[index]?;
                        Some(VmgValue {
                            angle_rad: *angle,
                            speed_mps: speed * angle.cos().abs(),
                        })
                    })
                    .max_by(|left, right| left.speed_mps.total_cmp(&right.speed_mps))
            };
            VmgSample {
                tws_mps: *tws,
                beat: best_vmg(true),
                run: best_vmg(false),
            }
        })
        .collect()
}

fn no_sail_taper(twa_rad: f64, beat_angle: f64, run_angle: f64) -> f64 {
    if twa_rad < beat_angle {
        let zero_angle = beat_angle * 0.75;
        if twa_rad <= zero_angle {
            0.0
        } else {
            (twa_rad - zero_angle) / (beat_angle - zero_angle)
        }
    } else if twa_rad > run_angle {
        let remaining = PI - run_angle;
        if remaining <= 0.0 {
            0.0
        } else {
            ((PI - twa_rad) / remaining).clamp(0.0, 1.0)
        }
    } else {
        1.0
    }
}

fn bilinear(
    matrix: &[Vec<Option<f64>>],
    twa_lower: usize,
    twa_upper: usize,
    twa_fraction: f64,
    tws_lower: usize,
    tws_upper: usize,
    tws_fraction: f64,
) -> Option<f64> {
    let corners = [
        (
            twa_lower,
            tws_lower,
            (1.0 - twa_fraction) * (1.0 - tws_fraction),
        ),
        (twa_lower, tws_upper, (1.0 - twa_fraction) * tws_fraction),
        (twa_upper, tws_lower, twa_fraction * (1.0 - tws_fraction)),
        (twa_upper, tws_upper, twa_fraction * tws_fraction),
    ];
    let mut total = 0.0;
    for (twa_index, tws_index, weight) in corners {
        if weight == 0.0 {
            continue;
        }
        total += matrix[twa_index][tws_index]? * weight;
    }
    Some(total)
}

fn bracket(axis: &[f64], value: f64) -> (usize, usize, f64) {
    if value <= axis[0] {
        return (0, 0, 0.0);
    }
    let last = axis.len() - 1;
    if value >= axis[last] {
        return (last, last, 0.0);
    }
    let upper = axis.partition_point(|entry| *entry < value);
    let lower = upper - 1;
    let fraction = (value - axis[lower]) / (axis[upper] - axis[lower]);
    (lower, upper, fraction)
}

fn normalize_twa(twa_rad: f64) -> f64 {
    let absolute = twa_rad.abs().rem_euclid(2.0 * PI);
    if absolute > PI {
        2.0 * PI - absolute
    } else {
        absolute
    }
}

fn parse_axis_header(line: &str, axis_name: &str) -> Result<Vec<f64>, DomainError> {
    let tokens: Vec<&str> = line
        .split(|character: char| character.is_whitespace() || character == ',')
        .filter(|token| !token.is_empty())
        .collect();
    let start = if tokens.first().is_some_and(|token| {
        token.eq_ignore_ascii_case(axis_name) || token.eq_ignore_ascii_case("TWA")
    }) {
        1
    } else {
        0
    };
    tokens[start..]
        .iter()
        .map(|token| parse_required_number(token, axis_name))
        .collect()
}

fn parse_json_axis(value: Option<&Value>, name: &str) -> Result<Vec<f64>, DomainError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_polar(format!("ORC JSON is missing {name}")))?;
    if values.len() < 2 {
        return Err(invalid_polar(format!("{name} needs at least two values")));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| invalid_polar(format!("{name} values must be numbers")))
        })
        .collect()
}

fn parse_speed_unit(unit: &str) -> Result<f64, DomainError> {
    match unit {
        "knots" => Ok(KNOT_TO_MPS),
        "ms" => Ok(1.0),
        _ => Err(invalid_polar(format!("unsupported speed unit {unit}"))),
    }
}

fn parse_json_speed_cell(value: &Value, factor: f64) -> Option<f64> {
    let value = value.as_f64()?;
    valid_speed(value).then_some(value * factor)
}

fn parse_speed_cell(token: &str) -> Option<f64> {
    let value = token.parse::<f64>().ok()?;
    valid_speed(value).then_some(value)
}

fn valid_speed(speed: f64) -> bool {
    speed.is_finite() && speed >= 0.0 && speed != INVALID_SPEED_SENTINEL
}

fn parse_required_number(token: &str, name: &str) -> Result<f64, DomainError> {
    let value = token
        .parse::<f64>()
        .map_err(|_| invalid_polar(format!("invalid {name} value {token:?}")))?;
    if !value.is_finite() {
        return Err(invalid_polar(format!("{name} values must be finite")));
    }
    Ok(value)
}

fn validate_axis(axis: &[f64], name: &str) -> Result<(), DomainError> {
    if axis.len() < 2 || axis.iter().any(|value| !value.is_finite()) {
        return Err(invalid_polar(format!(
            "{name} axis needs at least two finite values"
        )));
    }
    if axis.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(invalid_polar(format!(
            "{name} axis must be strictly increasing"
        )));
    }
    Ok(())
}

fn sort_dedup_axis(axis: &mut Vec<f64>) {
    axis.sort_by(f64::total_cmp);
    axis.dedup_by(|left, right| *left == *right);
}

fn assemble_csv_sails(
    twa_axis: &[f64],
    tws_axis: &[f64],
    sail_ids: &[String],
    records: Vec<(f64, f64, Option<f64>, String)>,
) -> Result<Vec<CsvSailMatrix>, DomainError> {
    let mut records_by_sail: HashMap<String, Vec<(f64, f64, Option<f64>)>> = HashMap::new();
    for (twa, tws, speed, sail_id) in records {
        records_by_sail
            .entry(sail_id)
            .or_default()
            .push((twa, tws, speed));
    }
    let mut output = Vec::with_capacity(sail_ids.len());
    for sail_id in sail_ids {
        let mut matrix = vec![vec![None; tws_axis.len()]; twa_axis.len()];
        let mut seen = vec![false; twa_axis.len() * tws_axis.len()];
        let sail_records = records_by_sail
            .get(sail_id)
            .ok_or_else(|| invalid_polar(format!("CSV has no rows for sail {sail_id}")))?;
        for (twa, tws, speed) in sail_records {
            let twa_index = twa_axis
                .binary_search_by(|value| value.total_cmp(twa))
                .map_err(|_| invalid_polar("CSV TWA axis lookup failed"))?;
            let tws_index = tws_axis
                .binary_search_by(|value| value.total_cmp(tws))
                .map_err(|_| invalid_polar("CSV TWS axis lookup failed"))?;
            let index = twa_index * tws_axis.len() + tws_index;
            if seen[index] {
                return Err(invalid_polar(format!(
                    "CSV repeats coordinate TWA={twa}, TWS={tws} for sail {sail_id}"
                )));
            }
            seen[index] = true;
            matrix[twa_index][tws_index] = speed.map(|value| value * KNOT_TO_MPS);
        }
        if seen.iter().any(|entry| !entry) {
            return Err(invalid_polar(format!(
                "CSV grid for sail {sail_id} is missing TWA/TWS combinations"
            )));
        }
        output.push((sail_id.clone(), matrix));
    }
    Ok(output)
}

fn lerp(lower: f64, upper: f64, fraction: f64) -> f64 {
    lower + (upper - lower) * fraction
}

fn invalid_polar(message: impl Into<String>) -> DomainError {
    DomainError::InvalidPolar(message.into())
}

#[cfg(test)]
mod tests {
    use super::{CsvFormat, Polar, PolarCache, KNOT_TO_MPS};
    use std::f64::consts::PI;

    const POLAR_POL: &str = "\
// Cruiser-Racer 30
# TWS in knots
TWS 6 10
40 4 8
90 5 10
140 4 8
";

    fn sample_polar() -> Polar {
        Polar::parse_pol(POLAR_POL).unwrap()
    }

    #[test]
    fn parses_pol_grid_and_converts_knots_to_si() {
        let polar = sample_polar();

        assert_eq!(polar.sails()[0].sail_id, "default");
        assert!((polar.tws_axis_mps()[0] - 6.0 * KNOT_TO_MPS).abs() < 1e-12);
        assert!((polar.twa_axis_rad()[0] - 40.0_f64.to_radians()).abs() < 1e-12);
        assert!((polar.sails()[0].speeds_mps[0][0].unwrap() - 4.0 * KNOT_TO_MPS).abs() < 1e-12);
    }

    #[test]
    fn interpolates_bilinearly_and_scales_low_wind_quadratically() {
        let polar = sample_polar();
        let speed = polar
            .speed_for_sail("default", 65.0_f64.to_radians(), 8.0 * KNOT_TO_MPS)
            .unwrap()
            .unwrap();
        assert!((speed - 6.75 * KNOT_TO_MPS).abs() < 1e-12);

        let low_wind = polar
            .speed_for_sail("default", PI / 2.0, 3.0 * KNOT_TO_MPS)
            .unwrap()
            .unwrap();
        assert!((low_wind - 1.25 * KNOT_TO_MPS).abs() < 1e-12);
    }

    #[test]
    fn tapers_beat_angle_and_computes_vmg_and_sail_crossovers() {
        let polar = sample_polar();
        let dead_zone = polar
            .speed_for_sail("default", 20.0_f64.to_radians(), 6.0 * KNOT_TO_MPS)
            .unwrap()
            .unwrap();
        assert_eq!(dead_zone, 0.0);
        let tapered = polar
            .speed_for_sail("default", 35.0_f64.to_radians(), 6.0 * KNOT_TO_MPS)
            .unwrap()
            .unwrap();
        assert!((tapered - 2.0 * KNOT_TO_MPS).abs() < 1e-12);

        assert!(
            (polar.vmg_table()[0].beat.as_ref().unwrap().angle_rad - 40.0_f64.to_radians()).abs()
                < 1e-12
        );
        assert!(
            (polar.vmg_table()[0].run.as_ref().unwrap().angle_rad - 140.0_f64.to_radians()).abs()
                < 1e-12
        );
        assert_eq!(polar.crossover_matrix()[0][0], Some("default"));
    }

    #[test]
    fn parses_orc_json_with_independent_speed_units_and_sail_configurations() {
        let json = r#"{
          "metadata":{"boatName":"Test yacht","unitTWS":"ms","unitBS":"knots"},
          "tws_grid":[2,4],
          "twa_grid":[40,90,140],
          "configurations":[
            {"sail_id":"J1","description":"Jib","matrix":[[4,8],[5,10],[4,8]]},
            {"sail_id":"A1","description":"Asymmetric","matrix":[[3,6],[6,11],[7,12]]}
          ]
        }"#;
        let polar = Polar::parse_orc_json(json).unwrap();
        assert_eq!(polar.sails().len(), 2);
        assert!((polar.tws_axis_mps()[0] - 2.0).abs() < 1e-12);
        assert!((polar.sails()[0].speeds_mps[0][0].unwrap() - 4.0 * KNOT_TO_MPS).abs() < 1e-12);
        assert_eq!(polar.crossover_matrix()[2][1], Some("A1"));
    }

    #[test]
    fn parses_normalized_and_explicit_matrix_csv_formats() {
        let normalized = "\
TWA,TWS,BS,SailID
40,6,4,J1
40,10,8,J1
90,6,5,J1
90,10,10,J1
140,6,4,J1
140,10,8,J1
";
        let polar = Polar::parse_csv(normalized, CsvFormat::Normalized).unwrap();
        assert_eq!(polar.sails()[0].sail_id, "J1");

        let matrix = "TWA,6,10\n40,4,8\n90,5,10\n140,4,8\n";
        let matrix_polar = Polar::parse_csv(matrix, CsvFormat::Matrix).unwrap();
        assert_eq!(matrix_polar.sails()[0].sail_id, "default");
        assert_eq!(matrix_polar.twa_axis_rad(), polar.twa_axis_rad());
    }

    #[test]
    fn treats_invalid_cells_as_unreachable_and_rejects_incomplete_csv_grids() {
        let input = "TWS 6 10\n40 4 99.9\n60 2 3\n90 -1 NaN\n140 4 8\n";
        let polar = Polar::parse_pol(input).unwrap();
        assert_eq!(polar.sails()[0].speeds_mps[0][1], None);
        assert_eq!(polar.sails()[0].speeds_mps[2][0], None);
        assert_eq!(polar.sails()[0].speeds_mps[2][1], None);

        let incomplete = "TWA,TWS,BS,SailID\n40,6,4,J1\n90,6,5,J1\n";
        assert!(Polar::parse_csv(incomplete, CsvFormat::Normalized).is_err());
    }

    #[test]
    fn serializes_and_validates_cached_polar_payloads() {
        let polar = sample_polar();
        let encoded = serde_json::to_string(&polar).unwrap();
        let decoded: Polar = serde_json::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, polar);
        assert_eq!(Polar::cache_key("boat-42").unwrap(), "vpp:boat-42");
    }

    #[test]
    #[ignore = "requires a Redis server and REDIS_TEST_URL"]
    fn redis_cache_round_trip_preserves_polar() {
        let url = std::env::var("REDIS_TEST_URL").unwrap();
        let client = redis::Client::open(url).unwrap();
        let mut connection = client.get_connection().unwrap();
        let vessel_id = format!("polar-test-{}", std::process::id());
        let key = Polar::cache_key(&vessel_id).unwrap();
        let polar = sample_polar();

        PolarCache::set(&mut connection, &vessel_id, &polar).unwrap();
        assert_eq!(
            PolarCache::get(&mut connection, &vessel_id).unwrap(),
            Some(polar)
        );
        redis::cmd("DEL")
            .arg(key)
            .query::<()>(&mut connection)
            .unwrap();
    }
}
