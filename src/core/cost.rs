use std::f64::consts::{PI, TAU};

use crate::error::DomainError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeCostConfig {
    pub tack_penalty_s: f64,
    pub gybe_penalty_s: f64,
    pub sail_change_penalty_s: f64,
    pub fuel_weight_s_per_l: f64,
    pub wave_weight_s_per_m: f64,
    pub wave_comfort_height_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeCostInput<'a> {
    pub elapsed_s: f64,
    pub previous_twa_rad: f64,
    pub current_twa_rad: f64,
    pub previous_sail: &'a str,
    pub current_sail: &'a str,
    pub fuel_used_l: f64,
    pub wave_height_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManeuverChanges {
    pub tack: bool,
    pub gybe: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeCostBreakdown {
    pub elapsed_s: f64,
    pub tack_penalty_s: f64,
    pub gybe_penalty_s: f64,
    pub sail_change_penalty_s: f64,
    pub fuel_penalty_s: f64,
    pub wave_penalty_s: f64,
    pub total_s: f64,
    pub maneuver_changes: ManeuverChanges,
    pub sail_changed: bool,
}

pub fn detect_maneuvers(
    previous_twa_rad: f64,
    current_twa_rad: f64,
) -> Result<ManeuverChanges, DomainError> {
    if !previous_twa_rad.is_finite() || !current_twa_rad.is_finite() {
        return Err(invalid_input("TWA values must be finite"));
    }

    let previous = normalize_relative(previous_twa_rad);
    let current = normalize_relative(current_twa_rad);
    let crossed_wind_axis = (previous < 0.0 && current > 0.0) || (previous > 0.0 && current < 0.0);
    if !crossed_wind_axis {
        return Ok(ManeuverChanges {
            tack: false,
            gybe: false,
        });
    }

    if (current - previous).abs() > PI {
        Ok(ManeuverChanges {
            tack: false,
            gybe: true,
        })
    } else {
        Ok(ManeuverChanges {
            tack: true,
            gybe: false,
        })
    }
}

pub fn evaluate_edge_cost(
    input: EdgeCostInput<'_>,
    config: EdgeCostConfig,
) -> Result<EdgeCostBreakdown, DomainError> {
    validate_nonnegative(input.elapsed_s, "elapsed_s")?;
    validate_nonnegative(input.fuel_used_l, "fuel_used_l")?;
    validate_nonnegative(input.wave_height_m, "wave_height_m")?;
    validate_nonnegative(config.tack_penalty_s, "tack_penalty_s")?;
    validate_nonnegative(config.gybe_penalty_s, "gybe_penalty_s")?;
    validate_nonnegative(config.sail_change_penalty_s, "sail_change_penalty_s")?;
    validate_nonnegative(config.fuel_weight_s_per_l, "fuel_weight_s_per_l")?;
    validate_nonnegative(config.wave_weight_s_per_m, "wave_weight_s_per_m")?;
    validate_nonnegative(config.wave_comfort_height_m, "wave_comfort_height_m")?;

    let maneuver_changes = detect_maneuvers(input.previous_twa_rad, input.current_twa_rad)?;
    let sail_changed = input.previous_sail != input.current_sail;
    let tack_penalty_s = if maneuver_changes.tack {
        config.tack_penalty_s
    } else {
        0.0
    };
    let gybe_penalty_s = if maneuver_changes.gybe {
        config.gybe_penalty_s
    } else {
        0.0
    };
    let sail_change_penalty_s = if sail_changed {
        config.sail_change_penalty_s
    } else {
        0.0
    };
    let fuel_penalty_s = config.fuel_weight_s_per_l * input.fuel_used_l;
    let wave_penalty_s =
        config.wave_weight_s_per_m * (input.wave_height_m - config.wave_comfort_height_m).max(0.0);
    let total_s = input.elapsed_s
        + tack_penalty_s
        + gybe_penalty_s
        + sail_change_penalty_s
        + fuel_penalty_s
        + wave_penalty_s;

    Ok(EdgeCostBreakdown {
        elapsed_s: input.elapsed_s,
        tack_penalty_s,
        gybe_penalty_s,
        sail_change_penalty_s,
        fuel_penalty_s,
        wave_penalty_s,
        total_s,
        maneuver_changes,
        sail_changed,
    })
}

fn normalize_relative(angle: f64) -> f64 {
    (angle + PI).rem_euclid(TAU) - PI
}

fn validate_nonnegative(value: f64, name: &str) -> Result<(), DomainError> {
    if !value.is_finite() || value < 0.0 {
        return Err(invalid_input(format!(
            "{name} must be finite and non-negative"
        )));
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> DomainError {
    DomainError::InvalidCostInput(message.into())
}

#[cfg(test)]
mod tests {
    use super::{
        detect_maneuvers, evaluate_edge_cost, EdgeCostConfig, EdgeCostInput, ManeuverChanges,
    };

    fn config() -> EdgeCostConfig {
        EdgeCostConfig {
            tack_penalty_s: 300.0,
            gybe_penalty_s: 180.0,
            sail_change_penalty_s: 60.0,
            fuel_weight_s_per_l: 120.0,
            wave_weight_s_per_m: 20.0,
            wave_comfort_height_m: 1.5,
        }
    }

    #[test]
    fn detects_tacks_across_the_wind_axis_and_gybes_across_the_downwind_axis() {
        assert_eq!(
            detect_maneuvers(40.0_f64.to_radians(), -40.0_f64.to_radians()).unwrap(),
            ManeuverChanges {
                tack: true,
                gybe: false,
            }
        );
        assert_eq!(
            detect_maneuvers(170.0_f64.to_radians(), -170.0_f64.to_radians()).unwrap(),
            ManeuverChanges {
                tack: false,
                gybe: true,
            }
        );
        assert_eq!(
            detect_maneuvers(40.0_f64.to_radians(), 70.0_f64.to_radians()).unwrap(),
            ManeuverChanges {
                tack: false,
                gybe: false,
            }
        );
    }

    #[test]
    fn accumulates_elapsed_maneuver_fuel_and_wave_costs_exactly() {
        let cost = evaluate_edge_cost(
            EdgeCostInput {
                elapsed_s: 3_600.0,
                previous_twa_rad: 40.0_f64.to_radians(),
                current_twa_rad: -40.0_f64.to_radians(),
                previous_sail: "J1",
                current_sail: "A1",
                fuel_used_l: 1.5,
                wave_height_m: 3.0,
            },
            config(),
        )
        .unwrap();

        assert!(cost.maneuver_changes.tack);
        assert!(!cost.maneuver_changes.gybe);
        assert!(cost.sail_changed);
        assert_eq!(cost.tack_penalty_s, 300.0);
        assert_eq!(cost.gybe_penalty_s, 0.0);
        assert_eq!(cost.sail_change_penalty_s, 60.0);
        assert_eq!(cost.fuel_penalty_s, 180.0);
        assert_eq!(cost.wave_penalty_s, 30.0);
        assert_eq!(cost.total_s, 4_170.0);
    }

    #[test]
    fn applies_gybe_penalty_and_omits_wave_penalty_below_comfort_height() {
        let cost = evaluate_edge_cost(
            EdgeCostInput {
                elapsed_s: 100.0,
                previous_twa_rad: 175.0_f64.to_radians(),
                current_twa_rad: -175.0_f64.to_radians(),
                previous_sail: "A1",
                current_sail: "A1",
                fuel_used_l: 0.0,
                wave_height_m: 1.0,
            },
            config(),
        )
        .unwrap();

        assert_eq!(cost.gybe_penalty_s, 180.0);
        assert_eq!(cost.tack_penalty_s, 0.0);
        assert_eq!(cost.wave_penalty_s, 0.0);
        assert_eq!(cost.total_s, 280.0);
    }

    #[test]
    fn rejects_non_finite_or_negative_cost_inputs() {
        let invalid = EdgeCostInput {
            elapsed_s: f64::NAN,
            previous_twa_rad: 0.0,
            current_twa_rad: 0.0,
            previous_sail: "J1",
            current_sail: "J1",
            fuel_used_l: 0.0,
            wave_height_m: 0.0,
        };
        assert!(evaluate_edge_cost(invalid, config()).is_err());
        assert!(detect_maneuvers(0.0, f64::INFINITY).is_err());

        let negative_penalty = EdgeCostConfig {
            tack_penalty_s: -1.0,
            ..config()
        };
        assert!(evaluate_edge_cost(
            EdgeCostInput {
                elapsed_s: 1.0,
                previous_twa_rad: 0.0,
                current_twa_rad: 0.0,
                previous_sail: "J1",
                current_sail: "J1",
                fuel_used_l: 0.0,
                wave_height_m: 0.0,
            },
            negative_penalty
        )
        .is_err());
    }
}
