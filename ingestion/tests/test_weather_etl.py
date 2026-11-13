import json
from pathlib import Path

import numpy as np
import pytest
import xarray as xr
from ingestion.weather_etl import (
    _input_engine,
    ingest_weather,
)


def test_selects_netcdf_engine_and_grib_engine_from_input_extension() -> None:
    assert _input_engine("forecast.nc", None) is None
    assert _input_engine("forecast.grib2", None) == "cfgrib"
    assert _input_engine("forecast.unknown", "custom") == "custom"
    with pytest.raises(ValueError, match="unsupported"):
        _input_engine("forecast.unknown", None)


def test_grib2_weather_is_decoded_and_written_to_zarr(tmp_path: Path) -> None:
    eccodes = pytest.importorskip("eccodes")
    source_path = tmp_path / "weather.grib2"
    output_path = tmp_path / "weather-grib.zarr"

    with source_path.open("wb") as stream:
        for forecast_hour in (0, 6):
            handle = eccodes.codes_grib_new_from_samples("regular_ll_sfc_grib2")
            try:
                eccodes.codes_set(handle, "Ni", 3)
                eccodes.codes_set(handle, "Nj", 2)
                eccodes.codes_set(handle, "latitudeOfFirstGridPointInDegrees", 10.0)
                eccodes.codes_set(handle, "longitudeOfFirstGridPointInDegrees", 0.0)
                eccodes.codes_set(handle, "latitudeOfLastGridPointInDegrees", 0.0)
                eccodes.codes_set(handle, "longitudeOfLastGridPointInDegrees", 2.0)
                eccodes.codes_set(handle, "iDirectionIncrementInDegrees", 1.0)
                eccodes.codes_set(handle, "jDirectionIncrementInDegrees", 10.0)
                eccodes.codes_set(handle, "dataDate", 20260101)
                eccodes.codes_set(handle, "dataTime", 0)
                eccodes.codes_set(handle, "forecastTime", forecast_hour)
                eccodes.codes_set(handle, "shortName", "u")
                eccodes.codes_set_values(handle, [1.0] * 6)
                eccodes.codes_write(handle, stream)
            finally:
                eccodes.codes_release(handle)

    ingest_weather(
        source_path,
        str(output_path),
        variables=["u"],
        time_dim="step",
    )

    with xr.open_zarr(output_path, consolidated=True) as result:
        assert result["u"].dims == ("lat", "lon", "time")
        assert result.sizes["time"] == 2
        assert np.all(result["u"].values == 1.0)
        assert np.array_equal(
            result["time"].values,
            np.array([1767225600, 1767247200], dtype=np.int64),
        )


def test_netcdf_weather_is_written_with_canonical_dimensions_and_chunks(
    tmp_path: Path,
) -> None:
    source_path = tmp_path / "weather.nc"
    output_path = tmp_path / "weather.zarr"
    source = xr.Dataset(
        {
            "wind_u": (
                ("latitude", "longitude", "time"),
                np.arange(4 * 3 * 2, dtype=np.float32).reshape(4, 3, 2),
            ),
            "wind_v": (
                ("time", "latitude", "longitude"),
                np.ones((2, 4, 3), dtype=np.float32),
            ),
            "land_masked": (
                ("latitude", "longitude", "time"),
                np.full((4, 3, 2), np.nan, dtype=np.float32),
            ),
        },
        coords={
            "latitude": [0.0, 1.0, 2.0, 3.0],
            "longitude": [10.0, 11.0, 12.0],
            "time": np.array(["2026-01-01", "2026-01-01T06:00"], dtype="datetime64"),
        },
    )
    source.to_netcdf(source_path)

    ingest_weather(source_path, str(output_path))

    with xr.open_zarr(output_path, consolidated=True) as result:
        assert result["wind_u"].dims == ("lat", "lon", "time")
        assert result["wind_v"].dims == ("lat", "lon", "time")
        assert np.array_equal(result["wind_u"].values, source["wind_u"].values)
        assert np.isnan(result["land_masked"].values).all()
        assert result["wind_u"].encoding["chunks"] == (4, 3, 2)
    wind_metadata = json.loads((output_path / "wind_u" / ".zarray").read_text())
    assert wind_metadata["compressor"] is None


def test_rejects_weather_variables_with_extra_dimensions(tmp_path: Path) -> None:
    source_path = tmp_path / "weather.nc"
    source = xr.Dataset(
        {
            "wind": (
                ("level", "latitude", "longitude", "time"),
                np.ones((2, 1, 1, 1), dtype=np.float32),
            )
        },
        coords={
            "level": [1000, 850],
            "latitude": [0.0],
            "longitude": [0.0],
            "time": np.array(["2026-01-01"], dtype="datetime64"),
        },
    )
    source.to_netcdf(source_path)

    with pytest.raises(ValueError, match="no variables"):
        ingest_weather(source_path, str(tmp_path / "out.zarr"))


def test_large_dataset_uses_required_100_by_100_by_240_chunks(tmp_path: Path) -> None:
    source_path = tmp_path / "large_weather.nc"
    output_path = tmp_path / "large_weather.zarr"
    source = xr.Dataset(
        {
            "wind_u": (
                ("latitude", "longitude", "time"),
                np.zeros((101, 101, 241), dtype=np.float32),
            )
        },
        coords={
            "latitude": np.arange(101),
            "longitude": np.arange(101),
            "time": np.datetime64("2026-01-01")
            + np.arange(241).astype("timedelta64[h]"),
        },
    )
    source.to_netcdf(source_path)

    ingest_weather(source_path, str(output_path))

    with xr.open_zarr(output_path, consolidated=True) as result:
        assert result["wind_u"].encoding["chunks"] == (100, 100, 240)
