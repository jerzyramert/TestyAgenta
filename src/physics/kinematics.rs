use std::f64::consts::{PI, TAU};

use crate::error::DomainError;

const MIN_STW_FOR_LEEWAY_MPS: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrueWind {
    pub speed_mps: f64,
    pub angle_rad: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Velocity {
    pub sog_mps: f64,
    pub cog_rad: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotorSailingConfig {
    pub min_motoring_sog_mps: f64,
    pub motoring_sog_mps: f64,
    pub fuel_rate_lph: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotorSailingOutcome {
    pub velocity: Velocity,
    pub is_motoring: bool,
    pub fuel_remaining_l: f64,
}

pub fn compute_true_wind(
    wind_u_mps: f64,
    wind_v_mps: f64,
    hdg_rad: f64,
) -> Result<TrueWind, DomainError> {
    if !wind_u_mps.is_finite() || !wind_v_mps.is_finite() || !hdg_rad.is_finite() {
        return Err(invalid_input("wind components and heading must be finite"));
    }

    let speed_mps = wind_u_mps.hypot(wind_v_mps);
    if speed_mps == 0.0 {
        return Ok(TrueWind {
            speed_mps,
            angle_rad: 0.0,
        });
    }
    let wind_from_bearing = (-wind_u_mps).atan2(-wind_v_mps);
    let relative_angle = (wind_from_bearing - hdg_rad + PI).rem_euclid(TAU) - PI;

    Ok(TrueWind {
        speed_mps,
        angle_rad: relative_angle,
    })
}

pub fn compose_velocity_vectors(
    stw_mps: f64,
    hdg_rad: f64,
    curr_u_mps: f64,
    curr_v_mps: f64,
    leeway_coeff: f64,
    heel_rad: f64,
) -> Result<Velocity, DomainError> {
    if !stw_mps.is_finite()
        || stw_mps < 0.0
        || !hdg_rad.is_finite()
        || !curr_u_mps.is_finite()
        || !curr_v_mps.is_finite()
        || !leeway_coeff.is_finite()
        || leeway_coeff < 0.0
        || !heel_rad.is_finite()
    {
        return Err(invalid_input(
            "STW, current, heading, leeway coefficient, and heel must be finite; STW and coefficient must be non-negative",
        ));
    }

    let heading = hdg_rad.rem_euclid(TAU);
    let stw_east = stw_mps * heading.sin();
    let stw_north = stw_mps * heading.cos();
    let denominator = stw_mps.max(MIN_STW_FOR_LEEWAY_MPS).powi(2);
    let leeway_speed = leeway_coeff * heel_rad / denominator;
    let leeway_east = leeway_speed * heading.cos();
    let leeway_north = -leeway_speed * heading.sin();
    let east = stw_east + leeway_east + curr_u_mps;
    let north = stw_north + leeway_north + curr_v_mps;

    Ok(velocity_from_components(east, north))
}

pub fn evaluate_motor_sailing(
    velocity: Velocity,
    hdg_rad: f64,
    fuel_remaining_l: f64,
    elapsed_seconds: f64,
    config: MotorSailingConfig,
) -> Result<MotorSailingOutcome, DomainError> {
    if !velocity.sog_mps.is_finite()
        || velocity.sog_mps < 0.0
        || !velocity.cog_rad.is_finite()
        || !hdg_rad.is_finite()
        || !fuel_remaining_l.is_finite()
        || fuel_remaining_l < 0.0
        || !elapsed_seconds.is_finite()
        || elapsed_seconds < 0.0
        || !config.min_motoring_sog_mps.is_finite()
        || config.min_motoring_sog_mps < 0.0
        || !config.motoring_sog_mps.is_finite()
        || config.motoring_sog_mps < config.min_motoring_sog_mps
        || !config.fuel_rate_lph.is_finite()
        || config.fuel_rate_lph <= 0.0
    {
        return Err(invalid_input("motor-sailing parameters are invalid"));
    }

    let fuel_required_l = config.fuel_rate_lph * elapsed_seconds / 3_600.0;
    let can_motor = velocity.sog_mps < config.min_motoring_sog_mps
        && fuel_remaining_l > 0.0
        && fuel_remaining_l >= fuel_required_l;
    if can_motor {
        Ok(MotorSailingOutcome {
            velocity: Velocity {
                sog_mps: config.motoring_sog_mps,
                cog_rad: hdg_rad.rem_euclid(TAU),
            },
            is_motoring: true,
            fuel_remaining_l: (fuel_remaining_l - fuel_required_l).max(0.0),
        })
    } else {
        Ok(MotorSailingOutcome {
            velocity,
            is_motoring: false,
            fuel_remaining_l,
        })
    }
}

fn velocity_from_components(east_mps: f64, north_mps: f64) -> Velocity {
    let sog_mps = east_mps.hypot(north_mps);
    let cog_rad = if sog_mps == 0.0 {
        0.0
    } else {
        east_mps.atan2(north_mps).rem_euclid(TAU)
    };
    Velocity { sog_mps, cog_rad }
}

fn invalid_input(message: &str) -> DomainError {
    DomainError::InvalidKinematics(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        compose_velocity_vectors, compute_true_wind, evaluate_motor_sailing, MotorSailingConfig,
        Velocity,
    };
    use std::f64::consts::PI;

    #[test]
    fn computes_true_wind_relative_to_heading() {
        let wind = compute_true_wind(10.0, 0.0, 0.0).unwrap();
        assert_eq!(wind.speed_mps, 10.0);
        assert!((wind.angle_rad + PI / 2.0).abs() < 1e-12);
    }

    #[test]
    fn golden_current_drift_matches_expected_sog_and_cog() {
        let velocity = compose_velocity_vectors(
            5.0 * 1_852.0 / 3_600.0,
            0.0,
            2.0 * 1_852.0 / 3_600.0,
            0.0,
            0.0,
            0.0,
        )
        .unwrap();
        let expected_sog = 5.385_164_807_134_504 * 1_852.0 / 3_600.0;
        let expected_cog = (2.0_f64 / 5.0).atan();

        assert!((velocity.sog_mps - expected_sog).abs() < 0.001);
        assert!((velocity.cog_rad - expected_cog).abs() < 0.001);
    }

    #[test]
    fn applies_signed_leeway_and_clamps_low_stw_denominator() {
        let velocity = compose_velocity_vectors(0.0, 0.0, 0.0, 0.0, 0.1, 0.1).unwrap();
        assert!((velocity.sog_mps - 0.16).abs() < 1e-12);
        assert!((velocity.cog_rad - PI / 2.0).abs() < 1e-12);
    }

    #[test]
    fn starts_motor_and_deducts_fuel_only_when_sog_is_low_and_fuel_is_sufficient() {
        let outcome = evaluate_motor_sailing(
            Velocity {
                sog_mps: 1.0,
                cog_rad: 0.25,
            },
            PI / 2.0,
            1.0,
            3_600.0,
            MotorSailingConfig {
                min_motoring_sog_mps: 2.0,
                motoring_sog_mps: 2.5,
                fuel_rate_lph: 1.0,
            },
        )
        .unwrap();
        assert!(outcome.is_motoring);
        assert_eq!(outcome.velocity.sog_mps, 2.5);
        assert!((outcome.velocity.cog_rad - PI / 2.0).abs() < 1e-12);
        assert_eq!(outcome.fuel_remaining_l, 0.0);
    }

    #[test]
    fn fuel_exhaustion_or_insufficient_fuel_preserves_sailing_velocity() {
        let velocity = Velocity {
            sog_mps: 1.0,
            cog_rad: 0.25,
        };
        let config = MotorSailingConfig {
            min_motoring_sog_mps: 2.0,
            motoring_sog_mps: 2.5,
            fuel_rate_lph: 1.0,
        };

        for fuel in [0.0, 0.5] {
            let outcome = evaluate_motor_sailing(velocity, 1.0, fuel, 3_600.0, config).unwrap();
            assert!(!outcome.is_motoring);
            assert_eq!(outcome.velocity, velocity);
            assert_eq!(outcome.fuel_remaining_l, fuel);
        }
    }

    #[test]
    fn motor_threshold_is_strict_and_invalid_inputs_are_reported() {
        let config = MotorSailingConfig {
            min_motoring_sog_mps: 2.0,
            motoring_sog_mps: 2.5,
            fuel_rate_lph: 1.0,
        };
        let at_threshold = evaluate_motor_sailing(
            Velocity {
                sog_mps: 2.0,
                cog_rad: 0.0,
            },
            0.0,
            1.0,
            0.0,
            config,
        )
        .unwrap();
        assert!(!at_threshold.is_motoring);

        assert!(compose_velocity_vectors(-1.0, 0.0, 0.0, 0.0, 0.0, 0.0).is_err());
    }
}
