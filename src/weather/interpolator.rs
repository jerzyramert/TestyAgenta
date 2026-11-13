use std::path::Path;

use crate::error::DomainError;
use crate::types::WeatherSample;

use super::zarr_mmap::MmapZarrArray;

#[derive(Debug, Clone)]
pub struct WeatherVariables {
    pub wind_u: String,
    pub wind_v: String,
    pub gust: String,
    pub current_u: String,
    pub current_v: String,
    pub wave_height: String,
    pub wave_period: String,
    pub wave_dir: String,
}

impl WeatherVariables {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wind_u: impl Into<String>,
        wind_v: impl Into<String>,
        gust: impl Into<String>,
        current_u: impl Into<String>,
        current_v: impl Into<String>,
        wave_height: impl Into<String>,
        wave_period: impl Into<String>,
        wave_dir: impl Into<String>,
    ) -> Self {
        Self {
            wind_u: wind_u.into(),
            wind_v: wind_v.into(),
            gust: gust.into(),
            current_u: current_u.into(),
            current_v: current_v.into(),
            wave_height: wave_height.into(),
            wave_period: wave_period.into(),
            wave_dir: wave_dir.into(),
        }
    }

    fn names(&self) -> [&str; 8] {
        [
            &self.wind_u,
            &self.wind_v,
            &self.gust,
            &self.current_u,
            &self.current_v,
            &self.wave_height,
            &self.wave_period,
            &self.wave_dir,
        ]
    }
}

pub struct WeatherGrid4D {
    latitudes: Vec<f64>,
    longitudes: Vec<f64>,
    times_unix: Vec<i64>,
    fields: [MmapZarrArray; 8],
    unit_factors: [f64; 8],
}

impl WeatherGrid4D {
    pub fn open_zarr(
        store_path: impl AsRef<Path>,
        variables: &WeatherVariables,
    ) -> Result<Self, DomainError> {
        let store_path = store_path.as_ref();
        let latitudes = read_float_axis(&store_path.join("lat"), "lat")?;
        let longitudes = read_float_axis(&store_path.join("lon"), "lon")?;
        let times_unix = read_time_axis(&store_path.join("time"))?;
        validate_axis(&latitudes)?;
        validate_axis(&longitudes)?;
        validate_time_axis(&times_unix)?;

        let fields = variables.names().map(|name| {
            let path = store_path.join(name);
            let field = MmapZarrArray::open(&path)?;
            if field.dimensions()? != ["lat", "lon", "time"] {
                return Err(DomainError::InvalidZarr(format!(
                    "weather variable {name} must have dimensions [lat, lon, time]"
                )));
            }
            if field.shape() != [latitudes.len(), longitudes.len(), times_unix.len()] {
                return Err(DomainError::InvalidZarr(format!(
                    "weather variable {name} does not match coordinate dimensions"
                )));
            }
            Ok(field)
        });
        let fields = fields.into_iter().collect::<Result<Vec<_>, _>>()?;
        let fields: [MmapZarrArray; 8] = fields.try_into().map_err(|_| {
            DomainError::InvalidZarr("expected eight configured weather variables".to_owned())
        })?;
        let mut unit_factors = [0.0; 8];
        for field in 0..5 {
            unit_factors[field] = speed_to_meters_per_second(&fields[field].units()?)?;
        }
        unit_factors[5] = distance_to_meters(&fields[5].units()?)?;
        unit_factors[6] = period_to_seconds(&fields[6].units()?)?;
        unit_factors[7] = direction_to_radians(&fields[7].units()?)?;

        Ok(Self {
            latitudes,
            longitudes,
            times_unix,
            fields,
            unit_factors,
        })
    }

    pub fn get_4d_point(
        &self,
        latitude: f64,
        longitude: f64,
        time_unix: i64,
    ) -> Result<WeatherSample, DomainError> {
        if !latitude.is_finite() || !longitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
            return Err(DomainError::WeatherOutOfBounds);
        }
        let longitude = (longitude + 180.0).rem_euclid(360.0) - 180.0;
        let lat = bracket(&self.latitudes, latitude, false)?;
        let lon = bracket(&self.longitudes, longitude, true)?;
        let time = bracket_time(&self.times_unix, time_unix)?;

        Ok(WeatherSample {
            wind_u: interpolate(&self.fields[0], lat, lon, time)? * self.unit_factors[0],
            wind_v: interpolate(&self.fields[1], lat, lon, time)? * self.unit_factors[1],
            gust: interpolate(&self.fields[2], lat, lon, time)? * self.unit_factors[2],
            current_u: interpolate(&self.fields[3], lat, lon, time)? * self.unit_factors[3],
            current_v: interpolate(&self.fields[4], lat, lon, time)? * self.unit_factors[4],
            wave_height: interpolate(&self.fields[5], lat, lon, time)? * self.unit_factors[5],
            wave_period: interpolate(&self.fields[6], lat, lon, time)? * self.unit_factors[6],
            wave_dir: interpolate_direction(&self.fields[7], lat, lon, time, self.unit_factors[7])?,
            is_climatology: false,
        })
    }

    pub fn time_bounds(&self) -> (i64, i64) {
        (
            self.times_unix[0],
            self.times_unix[self.times_unix.len() - 1],
        )
    }
}

fn speed_to_meters_per_second(units: &str) -> Result<f64, DomainError> {
    match units.trim().to_ascii_lowercase().as_str() {
        "m/s" | "m s-1" | "m s**-1" | "m s^-1" => Ok(1.0),
        "kn" | "knot" | "knots" => Ok(1_852.0 / 3_600.0),
        _ => Err(DomainError::InvalidZarr(format!(
            "unsupported speed units {units}"
        ))),
    }
}

fn distance_to_meters(units: &str) -> Result<f64, DomainError> {
    match units.trim().to_ascii_lowercase().as_str() {
        "m" | "meter" | "meters" | "metre" | "metres" => Ok(1.0),
        "cm" => Ok(0.01),
        _ => Err(DomainError::InvalidZarr(format!(
            "unsupported wave-height units {units}"
        ))),
    }
}

fn period_to_seconds(units: &str) -> Result<f64, DomainError> {
    match units.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" | "second" | "seconds" => Ok(1.0),
        "min" | "minute" | "minutes" => Ok(60.0),
        _ => Err(DomainError::InvalidZarr(format!(
            "unsupported wave-period units {units}"
        ))),
    }
}

fn direction_to_radians(units: &str) -> Result<f64, DomainError> {
    match units.trim().to_ascii_lowercase().as_str() {
        "rad" | "radian" | "radians" => Ok(1.0),
        "deg" | "degree" | "degrees" | "°" => Ok(std::f64::consts::PI / 180.0),
        _ => Err(DomainError::InvalidZarr(format!(
            "unsupported wave-direction units {units}"
        ))),
    }
}

#[derive(Clone, Copy)]
struct Bracket {
    lower: usize,
    upper: usize,
    fraction: f64,
}

fn read_float_axis(path: &Path, name: &str) -> Result<Vec<f64>, DomainError> {
    let array = MmapZarrArray::open(path)?;
    if array.dimensions()? != [name] || array.shape().len() != 1 {
        return Err(DomainError::InvalidZarr(format!(
            "{name} coordinate must be one-dimensional"
        )));
    }
    (0..array.shape()[0])
        .map(|index| array.value_f64(&[index]))
        .collect()
}

fn read_time_axis(path: &Path) -> Result<Vec<i64>, DomainError> {
    let array = MmapZarrArray::open(path)?;
    if array.dimensions()? != ["time"] || array.shape().len() != 1 {
        return Err(DomainError::InvalidZarr(
            "time coordinate must be one-dimensional".to_owned(),
        ));
    }
    (0..array.shape()[0])
        .map(|index| array.value_i64(&[index]))
        .collect()
}

fn validate_axis(axis: &[f64]) -> Result<(), DomainError> {
    if axis.is_empty() || axis.iter().any(|value| !value.is_finite()) {
        return Err(DomainError::InvalidZarr(
            "coordinate axes must contain finite values".to_owned(),
        ));
    }
    if axis.len() > 1 {
        let ascending = axis[1] > axis[0];
        if axis[1] == axis[0]
            || axis.windows(2).any(|pair| {
                if ascending {
                    pair[1] <= pair[0]
                } else {
                    pair[1] >= pair[0]
                }
            })
        {
            return Err(DomainError::InvalidZarr(
                "coordinate axes must be strictly monotonic".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_time_axis(axis: &[i64]) -> Result<(), DomainError> {
    if axis.is_empty() || axis.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(DomainError::InvalidZarr(
            "time coordinates must be non-empty and strictly increasing".to_owned(),
        ));
    }
    Ok(())
}

fn bracket(axis: &[f64], value: f64, cyclic_longitude: bool) -> Result<Bracket, DomainError> {
    let ascending = axis.len() == 1 || axis[axis.len() - 1] > axis[0];
    let ordered_value = |ordered_index: usize| {
        if ascending {
            axis[ordered_index]
        } else {
            axis[axis.len() - 1 - ordered_index]
        }
    };
    if axis.len() == 1 {
        if (value - axis[0]).abs() <= f64::EPSILON {
            return Ok(Bracket {
                lower: 0,
                upper: 0,
                fraction: 0.0,
            });
        }
        return Err(DomainError::WeatherOutOfBounds);
    }

    let first = ordered_value(0);
    let last = ordered_value(axis.len() - 1);
    let cyclic =
        cyclic_longitude && last - first + (last - first) / (axis.len() - 1) as f64 >= 359.0;
    let center = (first + last) / 2.0;
    let adjusted_value = if cyclic {
        value + ((center - value) / 360.0).round() * 360.0
    } else {
        value
    };
    let source_index = |ordered_index: usize| {
        if ascending {
            ordered_index
        } else {
            axis.len() - 1 - ordered_index
        }
    };

    if adjusted_value < first {
        if !cyclic {
            return Err(DomainError::WeatherOutOfBounds);
        }
        let wrapped_value = adjusted_value + 360.0;
        let fraction = (wrapped_value - last) / (first + 360.0 - last);
        return Ok(Bracket {
            lower: source_index(axis.len() - 1),
            upper: source_index(0),
            fraction,
        });
    }
    if adjusted_value > last {
        if !cyclic {
            return Err(DomainError::WeatherOutOfBounds);
        }
        let fraction = (adjusted_value - last) / (first + 360.0 - last);
        return Ok(Bracket {
            lower: source_index(axis.len() - 1),
            upper: source_index(0),
            fraction,
        });
    }

    let mut low = 0;
    let mut high = axis.len();
    while low < high {
        let middle = low + (high - low) / 2;
        if ordered_value(middle) < adjusted_value {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low == 0 {
        return Ok(Bracket {
            lower: source_index(0),
            upper: source_index(0),
            fraction: 0.0,
        });
    }
    let lower_ordered = low - 1;
    let lower_value = ordered_value(lower_ordered);
    if adjusted_value == ordered_value(low.min(axis.len() - 1)) {
        let index = low.min(axis.len() - 1);
        return Ok(Bracket {
            lower: source_index(index),
            upper: source_index(index),
            fraction: 0.0,
        });
    }
    let upper_ordered = low.min(axis.len() - 1);
    let upper_value = ordered_value(upper_ordered);
    Ok(Bracket {
        lower: source_index(lower_ordered),
        upper: source_index(upper_ordered),
        fraction: (adjusted_value - lower_value) / (upper_value - lower_value),
    })
}

fn bracket_time(axis: &[i64], time_unix: i64) -> Result<Bracket, DomainError> {
    if time_unix < axis[0] || time_unix > axis[axis.len() - 1] {
        return Err(DomainError::WeatherOutOfBounds);
    }
    let mut low = 0;
    let mut high = axis.len();
    while low < high {
        let middle = low + (high - low) / 2;
        if axis[middle] < time_unix {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if axis[low] == time_unix {
        return Ok(Bracket {
            lower: low,
            upper: low,
            fraction: 0.0,
        });
    }
    let lower = low - 1;
    Ok(Bracket {
        lower,
        upper: low,
        fraction: (time_unix - axis[lower]) as f64 / (axis[low] - axis[lower]) as f64,
    })
}

fn interpolate(
    array: &MmapZarrArray,
    latitude: Bracket,
    longitude: Bracket,
    time: Bracket,
) -> Result<f64, DomainError> {
    let mut result = 0.0;
    for (lat_index, lat_weight) in [
        (latitude.lower, 1.0 - latitude.fraction),
        (latitude.upper, latitude.fraction),
    ] {
        for (lon_index, lon_weight) in [
            (longitude.lower, 1.0 - longitude.fraction),
            (longitude.upper, longitude.fraction),
        ] {
            for (time_index, time_weight) in [
                (time.lower, 1.0 - time.fraction),
                (time.upper, time.fraction),
            ] {
                let weight = lat_weight * lon_weight * time_weight;
                if weight == 0.0 {
                    continue;
                }
                let value = array.value_f64(&[lat_index, lon_index, time_index])?;
                if !value.is_finite() {
                    return Err(DomainError::MissingWeatherData);
                }
                result += weight * value;
            }
        }
    }
    Ok(result)
}

fn interpolate_direction(
    array: &MmapZarrArray,
    latitude: Bracket,
    longitude: Bracket,
    time: Bracket,
    radians_per_unit: f64,
) -> Result<f64, DomainError> {
    let mut eastward = 0.0;
    let mut northward = 0.0;
    for (lat_index, lat_weight) in [
        (latitude.lower, 1.0 - latitude.fraction),
        (latitude.upper, latitude.fraction),
    ] {
        for (lon_index, lon_weight) in [
            (longitude.lower, 1.0 - longitude.fraction),
            (longitude.upper, longitude.fraction),
        ] {
            for (time_index, time_weight) in [
                (time.lower, 1.0 - time.fraction),
                (time.upper, time.fraction),
            ] {
                let weight = lat_weight * lon_weight * time_weight;
                if weight == 0.0 {
                    continue;
                }
                let direction =
                    array.value_f64(&[lat_index, lon_index, time_index])? * radians_per_unit;
                if !direction.is_finite() {
                    return Err(DomainError::MissingWeatherData);
                }
                eastward += weight * direction.sin();
                northward += weight * direction.cos();
            }
        }
    }
    if eastward == 0.0 && northward == 0.0 {
        return Err(DomainError::MissingWeatherData);
    }
    Ok(eastward.atan2(northward).rem_euclid(std::f64::consts::TAU))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::{bracket, WeatherGrid4D, WeatherVariables};
    use crate::error::DomainError;

    static NEXT_TEST_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestStore(PathBuf);

    impl TestStore {
        fn new() -> Self {
            let id = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("sailboat-weather-test-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn write_array(
        path: &Path,
        shape: &[usize],
        chunks: &[usize],
        dtype: &str,
        dimensions: &[&str],
        units: Option<&str>,
        value_at: impl Fn(&[usize]) -> f64,
    ) {
        fs::create_dir(path).unwrap();
        fs::write(
            path.join(".zarray"),
            serde_json::to_vec(&json!({
                "zarr_format": 2,
                "shape": shape,
                "chunks": chunks,
                "dtype": dtype,
                "compressor": null,
                "fill_value": "NaN",
                "order": "C",
                "filters": null,
                "dimension_separator": "."
            }))
            .unwrap(),
        )
        .unwrap();
        let attributes = if let Some(units) = units {
            json!({"_ARRAY_DIMENSIONS": dimensions, "units": units})
        } else {
            json!({"_ARRAY_DIMENSIONS": dimensions})
        };
        fs::write(
            path.join(".zattrs"),
            serde_json::to_vec(&attributes).unwrap(),
        )
        .unwrap();

        let chunk_grid: Vec<usize> = shape
            .iter()
            .zip(chunks)
            .map(|(size, chunk)| size.div_ceil(*chunk))
            .collect();
        let chunk_count = chunk_grid.iter().product();
        for linear_chunk in 0..chunk_count {
            let chunk_indices = unravel(linear_chunk, &chunk_grid);
            let chunk_key = chunk_indices
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(".");
            let chunk_size = chunks.iter().product::<usize>();
            let mut chunk_bytes = Vec::with_capacity(chunk_size * 8);
            for local_linear in 0..chunk_size {
                let local_indices = unravel(local_linear, chunks);
                let global_indices: Vec<usize> = local_indices
                    .iter()
                    .zip(&chunk_indices)
                    .zip(chunks)
                    .map(|((local, chunk_index), chunk_size)| local + chunk_index * chunk_size)
                    .collect();
                let value = if global_indices
                    .iter()
                    .zip(shape)
                    .any(|(index, size)| index >= size)
                {
                    0.0
                } else {
                    value_at(&global_indices)
                };
                match dtype {
                    "<f8" => chunk_bytes.extend_from_slice(&value.to_le_bytes()),
                    "<i8" => chunk_bytes.extend_from_slice(&(value as i64).to_le_bytes()),
                    _ => panic!("unsupported test dtype"),
                }
            }
            fs::write(path.join(chunk_key), chunk_bytes).unwrap();
        }
    }

    fn unravel(mut index: usize, shape: &[usize]) -> Vec<usize> {
        let mut result = vec![0; shape.len()];
        for axis in (0..shape.len()).rev() {
            result[axis] = index % shape[axis];
            index /= shape[axis];
        }
        result
    }

    fn create_test_store(path: &Path, include_missing_gust: bool) {
        write_array(
            &path.join("lat"),
            &[3],
            &[2],
            "<f8",
            &["lat"],
            None,
            |index| index[0] as f64,
        );
        write_array(
            &path.join("lon"),
            &[2],
            &[2],
            "<f8",
            &["lon"],
            None,
            |index| index[0] as f64,
        );
        write_array(
            &path.join("time"),
            &[2],
            &[2],
            "<i8",
            &["time"],
            None,
            |index| [0.0, 10.0][index[0]],
        );

        for field in 0..8 {
            let name = format!("f{field}");
            write_array(
                &path.join(&name),
                &[3, 2, 2],
                &[2, 2, 2],
                "<f8",
                &["lat", "lon", "time"],
                Some(match field {
                    0 => "knots",
                    5 => "m",
                    6 => "s",
                    7 => "degrees",
                    _ => "m s-1",
                }),
                |index| {
                    if field == 2 && include_missing_gust && index == [0, 0, 0] {
                        f64::NAN
                    } else if field == 7 {
                        if index[1] == 0 {
                            350.0
                        } else {
                            10.0
                        }
                    } else {
                        field as f64 * 10.0
                            + index[0] as f64
                            + 2.0 * index[1] as f64
                            + 3.0 * index[2] as f64
                    }
                },
            );
        }
    }

    fn test_variables() -> WeatherVariables {
        WeatherVariables::new("f0", "f1", "f2", "f3", "f4", "f5", "f6", "f7")
    }

    #[test]
    fn interpolates_all_fields_across_mmap_chunks_and_wraps_directions() {
        let store = TestStore::new();
        create_test_store(store.path(), false);
        let grid = WeatherGrid4D::open_zarr(store.path(), &test_variables()).unwrap();

        let sample = grid.get_4d_point(0.5, 0.5, 5).unwrap();
        assert!((sample.wind_u - 3.0 * 1_852.0 / 3_600.0).abs() < 1e-12);
        assert!((sample.wind_v - 13.0).abs() < 1e-12);
        assert!((sample.gust - 23.0).abs() < 1e-12);
        assert!((sample.current_u - 33.0).abs() < 1e-12);
        assert!((sample.current_v - 43.0).abs() < 1e-12);
        assert!((sample.wave_height - 53.0).abs() < 1e-12);
        assert!((sample.wave_period - 63.0).abs() < 1e-12);
        assert!(sample.wave_dir < 1e-12 || (sample.wave_dir - std::f64::consts::TAU).abs() < 1e-12);
        assert!(!sample.is_climatology);

        let sample = grid.get_4d_point(1.5, 0.5, 5).unwrap();
        assert!((sample.wind_u - 4.0 * 1_852.0 / 3_600.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_forecast_exhaustion_and_land_masked_weather() {
        let store = TestStore::new();
        create_test_store(store.path(), true);
        let grid = WeatherGrid4D::open_zarr(store.path(), &test_variables()).unwrap();

        assert_eq!(
            grid.get_4d_point(0.5, 0.5, 11),
            Err(DomainError::WeatherOutOfBounds)
        );
        assert_eq!(
            grid.get_4d_point(0.5, 0.5, 5),
            Err(DomainError::MissingWeatherData)
        );
    }

    #[test]
    fn brackets_global_longitudes_across_the_zero_to_360_seam() {
        let axis = [0.0, 90.0, 180.0, 270.0];
        let bracket = bracket(&axis, -45.0, true).unwrap();

        assert_eq!(bracket.lower, 3);
        assert_eq!(bracket.upper, 0);
        assert!((bracket.fraction - 0.5).abs() < 1e-12);
    }

    #[test]
    fn weather_queries_allocate_no_heap_memory() {
        let store = TestStore::new();
        create_test_store(store.path(), false);
        let grid = WeatherGrid4D::open_zarr(store.path(), &test_variables()).unwrap();

        crate::allocation_test::start();
        let first = grid.get_4d_point(0.5, 0.5, 5);
        let second = grid.get_4d_point(1.5, 0.5, 5);
        let allocation_count = crate::allocation_test::stop();

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(allocation_count, 0);
    }
}
