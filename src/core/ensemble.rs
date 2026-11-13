use std::collections::HashSet;

use rayon::prelude::*;

use crate::core::isochrone::{RouteResult, RouteStatus};
use crate::error::DomainError;

#[derive(Debug, Clone, PartialEq)]
pub struct DepartureCandidate {
    pub departure_time_unix: i64,
    pub total_cost_s: f64,
    pub route: RouteResult,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DepartureOptimization {
    pub candidates: Vec<DepartureCandidate>,
    pub best_index: usize,
}

impl DepartureOptimization {
    pub fn best(&self) -> Option<&DepartureCandidate> {
        self.candidates.get(self.best_index)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnsembleEvaluation {
    pub member_routes: Vec<Option<RouteResult>>,
    pub member_costs_s: Vec<Option<f64>>,
    pub cvar_s: f64,
    pub accepted: bool,
}

pub fn optimize_departures<F>(
    departure_times_unix: &[i64],
    calculate_route: F,
) -> Result<DepartureOptimization, DomainError>
where
    F: Fn(i64) -> Result<RouteResult, DomainError> + Sync,
{
    if departure_times_unix.is_empty() {
        return Err(DomainError::InvalidEnsembleInput(
            "at least one departure time is required".to_owned(),
        ));
    }
    let unique_times: HashSet<_> = departure_times_unix.iter().copied().collect();
    if unique_times.len() != departure_times_unix.len() {
        return Err(DomainError::InvalidEnsembleInput(
            "departure times must be unique".to_owned(),
        ));
    }

    let results: Vec<_> = departure_times_unix
        .par_iter()
        .map(|&departure_time_unix| {
            calculate_route(departure_time_unix).map(|route| (departure_time_unix, route))
        })
        .collect();

    let mut candidates = Vec::with_capacity(results.len());
    for result in results {
        let (departure_time_unix, route) = result?;
        if route.status == RouteStatus::NoSafeRouteFound {
            continue;
        }
        let total_cost_s = route
            .waypoints
            .last()
            .map(|waypoint| waypoint.cumulative_cost_s)
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
            .ok_or_else(|| {
                DomainError::InvalidEnsembleInput(
                    "completed routes must have a finite, non-negative terminal cost".to_owned(),
                )
            })?;
        candidates.push(DepartureCandidate {
            departure_time_unix,
            total_cost_s,
            route,
        });
    }
    if candidates.is_empty() {
        return Err(DomainError::NoSafeRouteFound);
    }

    let best_index = candidates
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            left.total_cost_s
                .total_cmp(&right.total_cost_s)
                .then_with(|| left.departure_time_unix.cmp(&right.departure_time_unix))
        })
        .map(|(index, _)| index)
        .ok_or(DomainError::NoSafeRouteFound)?;

    Ok(DepartureOptimization {
        candidates,
        best_index,
    })
}

pub fn evaluate_ensemble<F>(
    member_count: usize,
    confidence: f64,
    maximum_cvar_s: f64,
    calculate_member_route: F,
) -> Result<EnsembleEvaluation, DomainError>
where
    F: Fn(usize) -> Result<RouteResult, DomainError> + Send + Sync,
{
    if member_count == 0 {
        return Err(DomainError::InvalidEnsembleInput(
            "at least one ensemble member is required".to_owned(),
        ));
    }
    validate_risk_parameters(confidence, maximum_cvar_s)?;

    let results: Vec<_> = (0..member_count)
        .into_par_iter()
        .map(calculate_member_route)
        .collect();

    let mut member_routes = Vec::with_capacity(member_count);
    let mut member_costs_s = Vec::with_capacity(member_count);
    for result in results {
        let route = result?;
        let cost = if route.status == RouteStatus::Completed {
            Some(
                route
                    .waypoints
                    .last()
                    .map(|waypoint| waypoint.cumulative_cost_s)
                    .filter(|cost| cost.is_finite() && *cost >= 0.0)
                    .ok_or_else(|| {
                        DomainError::InvalidEnsembleInput(
                            "completed ensemble routes must have a finite, non-negative terminal cost"
                                .to_owned(),
                        )
                    })?,
            )
        } else {
            None
        };
        member_costs_s.push(cost);
        member_routes.push((cost.is_some()).then_some(route));
    }
    let cvar_s = cvar_tail_risk(&member_costs_s, confidence)?;

    Ok(EnsembleEvaluation {
        member_routes,
        member_costs_s,
        cvar_s,
        accepted: cvar_s <= maximum_cvar_s,
    })
}

pub fn cvar_tail_risk(member_costs_s: &[Option<f64>], confidence: f64) -> Result<f64, DomainError> {
    if member_costs_s.is_empty() {
        return Err(DomainError::InvalidEnsembleInput(
            "at least one ensemble cost is required".to_owned(),
        ));
    }
    if !confidence.is_finite() || !(0.0..1.0).contains(&confidence) {
        return Err(DomainError::InvalidEnsembleInput(
            "CVaR confidence must be finite and in [0, 1)".to_owned(),
        ));
    }

    let mut losses = Vec::with_capacity(member_costs_s.len());
    for cost in member_costs_s {
        match cost {
            Some(value) if value.is_finite() && *value >= 0.0 => losses.push(*value),
            Some(_) => {
                return Err(DomainError::InvalidEnsembleInput(
                    "ensemble costs must be finite and non-negative".to_owned(),
                ));
            }
            None => losses.push(f64::INFINITY),
        }
    }
    losses.sort_by(f64::total_cmp);

    let tail_count = (1.0 - confidence) * losses.len() as f64;
    let mut remaining = tail_count;
    let mut tail_sum = 0.0;
    for loss in losses.iter().rev() {
        if remaining <= 0.0 {
            break;
        }
        let fraction = remaining.min(1.0);
        if loss.is_infinite() {
            return Ok(f64::INFINITY);
        }
        tail_sum += loss * fraction;
        remaining -= fraction;
    }

    Ok(tail_sum / tail_count)
}

fn validate_risk_parameters(confidence: f64, maximum_cvar_s: f64) -> Result<(), DomainError> {
    if !maximum_cvar_s.is_finite() || maximum_cvar_s < 0.0 {
        return Err(DomainError::InvalidEnsembleInput(
            "maximum CVaR must be finite and non-negative".to_owned(),
        ));
    }
    if !confidence.is_finite() || !(0.0..1.0).contains(&confidence) {
        return Err(DomainError::InvalidEnsembleInput(
            "CVaR confidence must be finite and in [0, 1)".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{cvar_tail_risk, evaluate_ensemble, optimize_departures};
    use crate::core::isochrone::{RouteResult, RouteStatus, Waypoint};
    use crate::error::DomainError;
    use crate::types::GeoPoint;

    fn route(cost_s: f64, status: RouteStatus) -> RouteResult {
        RouteResult {
            status,
            waypoints: (status == RouteStatus::Completed)
                .then(|| Waypoint {
                    point: GeoPoint::new(0.0, 0.0).unwrap(),
                    time_unix: 0,
                    heading_rad: 0.0,
                    course_rad: 0.0,
                    sog_mps: 0.0,
                    fuel_remaining_l: 0.0,
                    sail_index: 0,
                    is_motoring: false,
                    cumulative_cost_s: cost_s,
                })
                .into_iter()
                .collect(),
            partial_isochrones: Vec::new(),
            warning_climatology_used: false,
        }
    }

    #[test]
    fn departure_matrix_runs_all_times_and_picks_lowest_cost_deterministically() {
        let result = optimize_departures(&[30, 10, 20], |departure| {
            Ok(route((departure - 20).abs() as f64, RouteStatus::Completed))
        })
        .unwrap();

        assert_eq!(result.candidates.len(), 3);
        assert_eq!(result.best().unwrap().departure_time_unix, 20);
        assert_eq!(result.best().unwrap().total_cost_s, 0.0);
    }

    #[test]
    fn departure_optimization_skips_routes_pruned_as_unsafe() {
        let result = optimize_departures(&[10, 20], |departure| {
            if departure == 10 {
                Ok(route(0.0, RouteStatus::NoSafeRouteFound))
            } else {
                Ok(route(100.0, RouteStatus::Completed))
            }
        })
        .unwrap();

        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.best().unwrap().departure_time_unix, 20);
    }

    #[test]
    fn cvar_averages_fractional_worst_tail_and_infeasible_members_are_infinite_risk() {
        let costs = [Some(10.0), Some(20.0), Some(30.0), Some(40.0)];
        assert_eq!(cvar_tail_risk(&costs, 0.75).unwrap(), 40.0);
        assert!((cvar_tail_risk(&costs, 0.625).unwrap() - 36.666_666_666_666_664).abs() < 1e-12);
        assert!(cvar_tail_risk(&[Some(10.0), None], 0.5)
            .unwrap()
            .is_infinite());
    }

    #[test]
    fn ensemble_rejects_candidate_when_storm_tail_exceeds_risk_limit() {
        let result = evaluate_ensemble(4, 0.75, 120.0, |member| {
            Ok(if member == 3 {
                route(200.0, RouteStatus::Completed)
            } else {
                route(50.0, RouteStatus::Completed)
            })
        })
        .unwrap();

        assert_eq!(result.cvar_s, 200.0);
        assert!(!result.accepted);
        assert_eq!(result.member_routes.len(), 4);
    }

    #[test]
    fn ensemble_propagates_member_calculation_errors() {
        let result = evaluate_ensemble(2, 0.5, 100.0, |member| {
            if member == 0 {
                Err(DomainError::NoSafeRouteFound)
            } else {
                Ok(route(10.0, RouteStatus::Completed))
            }
        });

        assert_eq!(result, Err(DomainError::NoSafeRouteFound));
    }

    #[test]
    fn departure_optimization_rejects_duplicate_timestamps() {
        let result = optimize_departures(&[10, 10], |_| Ok(route(1.0, RouteStatus::Completed)));

        assert!(matches!(result, Err(DomainError::InvalidEnsembleInput(_))));
    }
}
