use crate::error::DomainError;
use crate::types::WeatherSample;

pub fn regional_weight(
    signed_distance_m: f64,
    transition_width_m: f64,
) -> Result<f64, DomainError> {
    if !signed_distance_m.is_finite()
        || !transition_width_m.is_finite()
        || transition_width_m <= 0.0
    {
        return Err(DomainError::InvalidWeatherBlend(
            "distance must be finite and transition width must be finite and positive".to_owned(),
        ));
    }

    let scaled_distance = signed_distance_m / transition_width_m;
    let weight = if scaled_distance >= 0.0 {
        1.0 / (1.0 + (-scaled_distance).exp())
    } else {
        let exponential = scaled_distance.exp();
        exponential / (1.0 + exponential)
    };
    Ok(weight)
}

pub fn blend_weather_samples(
    global: WeatherSample,
    regional: WeatherSample,
    regional_weight: f64,
) -> Result<WeatherSample, DomainError> {
    validate_sample(global)?;
    validate_sample(regional)?;
    if !regional_weight.is_finite() || !(0.0..=1.0).contains(&regional_weight) {
        return Err(DomainError::InvalidWeatherBlend(
            "regional weight must be finite and within [0, 1]".to_owned(),
        ));
    }

    let global_weight = 1.0 - regional_weight;
    let wave_east =
        global_weight * global.wave_dir.sin() + regional_weight * regional.wave_dir.sin();
    let wave_north =
        global_weight * global.wave_dir.cos() + regional_weight * regional.wave_dir.cos();
    if wave_east.hypot(wave_north) <= f64::EPSILON {
        return Err(DomainError::InvalidWeatherBlend(
            "opposing wave directions have no defined blended direction".to_owned(),
        ));
    }

    Ok(WeatherSample {
        wind_u: blend(global.wind_u, regional.wind_u, regional_weight),
        wind_v: blend(global.wind_v, regional.wind_v, regional_weight),
        gust: blend(global.gust, regional.gust, regional_weight),
        current_u: blend(global.current_u, regional.current_u, regional_weight),
        current_v: blend(global.current_v, regional.current_v, regional_weight),
        wave_height: blend(global.wave_height, regional.wave_height, regional_weight),
        wave_period: blend(global.wave_period, regional.wave_period, regional_weight),
        wave_dir: wave_east
            .atan2(wave_north)
            .rem_euclid(std::f64::consts::TAU),
        is_climatology: global.is_climatology || regional.is_climatology,
    })
}

fn blend(global: f64, regional: f64, regional_weight: f64) -> f64 {
    global.mul_add(1.0 - regional_weight, regional * regional_weight)
}

fn validate_sample(sample: WeatherSample) -> Result<(), DomainError> {
    let fields = [
        sample.wind_u,
        sample.wind_v,
        sample.gust,
        sample.current_u,
        sample.current_v,
        sample.wave_height,
        sample.wave_period,
        sample.wave_dir,
    ];
    if fields.iter().any(|value| !value.is_finite())
        || sample.gust < 0.0
        || sample.wave_height < 0.0
        || sample.wave_period < 0.0
    {
        return Err(DomainError::InvalidWeatherBlend(
            "weather values must be finite and magnitudes must be non-negative".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::error::DomainError;
    use crate::types::WeatherSample;

    use super::{blend_weather_samples, regional_weight};

    fn sample(wave_dir: f64) -> WeatherSample {
        WeatherSample {
            wind_u: 1.0,
            wind_v: 2.0,
            gust: 3.0,
            current_u: 4.0,
            current_v: 5.0,
            wave_height: 6.0,
            wave_period: 7.0,
            wave_dir,
            is_climatology: false,
        }
    }

    #[test]
    fn sigmoid_weight_is_bounded_monotonic_and_smooth_at_the_boundary() {
        let at_boundary = regional_weight(0.0, 1_000.0).unwrap();
        let half_width = 0.1;
        let before = regional_weight(-half_width, 1_000.0).unwrap();
        let after = regional_weight(half_width, 1_000.0).unwrap();
        let left_slope = (at_boundary - before) / half_width;
        let right_slope = (after - at_boundary) / half_width;

        assert_eq!(at_boundary, 0.5);
        assert!(regional_weight(-1_000.0, 1_000.0).unwrap() < at_boundary);
        assert!(regional_weight(1_000.0, 1_000.0).unwrap() > at_boundary);
        assert!((left_slope - right_slope).abs() < 1e-9);
        assert!(regional_weight(f64::MAX, 1.0).unwrap() <= 1.0);
    }

    #[test]
    fn blends_scalar_fields_and_wave_direction_across_north() {
        let mut global = sample(359.0_f64.to_radians());
        global.wind_u = 2.0;
        global.current_u = 0.0;
        global.wave_height = 2.0;
        global.is_climatology = true;
        let mut regional = sample(1.0_f64.to_radians());
        regional.wind_u = 10.0;
        regional.current_u = 8.0;
        regional.wave_height = 6.0;

        let blended = blend_weather_samples(global, regional, 0.25).unwrap();

        assert_eq!(blended.wind_u, 4.0);
        assert_eq!(blended.current_u, 2.0);
        assert_eq!(blended.wave_height, 3.0);
        let expected_wave_dir = 359.5_f64.to_radians();
        let direction_error = (blended.wave_dir - expected_wave_dir + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        assert!(direction_error.abs() < 1e-4);
        assert!(blended.is_climatology);
    }

    #[test]
    fn reports_invalid_inputs_and_undefined_opposing_wave_directions() {
        assert!(matches!(
            regional_weight(0.0, 0.0),
            Err(DomainError::InvalidWeatherBlend(_))
        ));
        assert!(matches!(
            blend_weather_samples(sample(0.0), sample(0.0), f64::NAN),
            Err(DomainError::InvalidWeatherBlend(_))
        ));
        assert!(matches!(
            blend_weather_samples(sample(0.0), sample(std::f64::consts::PI), 0.5),
            Err(DomainError::InvalidWeatherBlend(_))
        ));
    }
}
