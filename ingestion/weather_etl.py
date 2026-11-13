import argparse
from collections.abc import Sequence
from pathlib import Path

import fsspec
import numpy as np
import xarray as xr

DEFAULT_CHUNKS = {"lat": 100, "lon": 100, "time": 240}
SUPPORTED_SUFFIXES = {".grb", ".grb2", ".grib", ".grib2", ".nc", ".nc4", ".netcdf"}


def _input_engine(input_path: str | Path, engine: str | None) -> str | None:
    if engine is not None:
        return engine
    suffix = Path(str(input_path).split("?", maxsplit=1)[0]).suffix.lower()
    if suffix not in SUPPORTED_SUFFIXES:
        raise ValueError(f"unsupported weather input format: {suffix or '<no extension>'}")
    return "cfgrib" if suffix in {".grb", ".grb2", ".grib", ".grib2"} else None


def _select_weather_dataset(
    dataset: xr.Dataset,
    *,
    latitude_dim: str,
    longitude_dim: str,
    time_dim: str,
    variables: Sequence[str] | None,
) -> xr.Dataset:
    source_dims = (latitude_dim, longitude_dim, time_dim)
    missing_dims = [dim for dim in source_dims if dim not in dataset.dims]
    if missing_dims:
        raise ValueError(f"input dataset is missing coordinate dimensions: {missing_dims}")

    candidates = [
        name
        for name, variable in dataset.data_vars.items()
        if set(variable.dims) == set(source_dims)
    ]
    selected_names = list(variables) if variables is not None else candidates
    if not selected_names:
        raise ValueError("input dataset contains no variables on the configured lat/lon/time grid")

    missing_variables = [name for name in selected_names if name not in dataset.data_vars]
    if missing_variables:
        raise ValueError(f"requested weather variables are missing: {missing_variables}")

    invalid_dims = {
        name: dataset[name].dims
        for name in selected_names
        if set(dataset[name].dims) != set(source_dims)
    }
    if invalid_dims:
        raise ValueError(
            "weather variables must use exactly the configured lat/lon/time dimensions: "
            f"{invalid_dims}"
        )

    effective_time_dim = time_dim
    valid_time = dataset.coords.get("valid_time")
    if (
        time_dim != "time"
        and valid_time is not None
        and valid_time.dims == (time_dim,)
    ):
        dataset = dataset.swap_dims({time_dim: "valid_time"})
        effective_time_dim = "valid_time"

    source_dims = (latitude_dim, longitude_dim, effective_time_dim)
    dim_renames = {
        source_name: canonical_name
        for source_name, canonical_name in zip(
            source_dims,
            ("lat", "lon", "time"),
            strict=True,
        )
        if source_name != canonical_name
    }
    result = dataset[selected_names]
    if "time" in result.coords and "time" not in source_dims and "time" in dim_renames.values():
        result = result.rename_vars({"time": "forecast_reference_time"})
    result = result.rename(dim_renames).transpose("lat", "lon", "time")
    time_values = result["time"].values
    if not np.issubdtype(time_values.dtype, np.datetime64):
        raise ValueError("weather time coordinate must decode to UTC datetime values")
    unix_seconds = time_values.astype("datetime64[s]").astype(np.int64)
    result = result.assign_coords(time=("time", unix_seconds))
    result["time"].attrs.clear()
    result["time"].encoding.clear()
    return result


def ingest_weather(
    input_path: str | Path,
    output_store: str,
    *,
    variables: Sequence[str] | None = None,
    engine: str | None = None,
    latitude_dim: str = "latitude",
    longitude_dim: str = "longitude",
    time_dim: str = "time",
) -> None:
    """Convert a 3D weather dataset into time-chunked Zarr arrays."""
    selected_engine = _input_engine(input_path, engine)
    backend_kwargs = {"indexpath": ""} if selected_engine == "cfgrib" else None
    if output_store.startswith("s3://"):
        store = fsspec.get_mapper(output_store, create=True)
    else:
        local_store = Path(output_store.removeprefix("file://"))
        local_store.mkdir(parents=True, exist_ok=True)
        store = str(local_store)

    with xr.open_dataset(
        input_path,
        engine=selected_engine,
        backend_kwargs=backend_kwargs,
    ) as source:
        dataset = _select_weather_dataset(
            source,
            latitude_dim=latitude_dim,
            longitude_dim=longitude_dim,
            time_dim=time_dim,
            variables=variables,
        )
        chunk_sizes = {
            dim: min(DEFAULT_CHUNKS[dim], dataset.sizes[dim])
            for dim in ("lat", "lon", "time")
        }
        for variable in dataset.variables.values():
            variable.encoding.clear()
        chunked = dataset.chunk(chunk_sizes)
        encoding = {
            name: {
                "chunks": tuple(chunk_sizes[dim] for dim in ("lat", "lon", "time")),
                "compressors": None,
            }
            for name in chunked.data_vars
        }
        encoding.update(
            {
                name: {"chunks": (chunk_sizes[name],), "compressors": None}
                for name in ("lat", "lon", "time")
            }
        )
        chunked.to_zarr(
            store,
            mode="w",
            encoding=encoding,
            consolidated=True,
            zarr_format=2,
        )


def main(argv: Sequence[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        description="Convert GRIB2 or NetCDF weather grids into time-chunked Zarr."
    )
    parser.add_argument("--input", required=True, help="Local input GRIB2 or NetCDF file")
    parser.add_argument("--output", required=True, help="Local Zarr path or s3:// bucket path")
    parser.add_argument("--engine", help="Explicit xarray engine override")
    parser.add_argument("--latitude-dim", default="latitude")
    parser.add_argument("--longitude-dim", default="longitude")
    parser.add_argument("--time-dim", default="time")
    parser.add_argument(
        "--variable",
        action="append",
        dest="variables",
        help="Weather variable to ingest; repeat to select multiple (default: all matching grids)",
    )
    args = parser.parse_args(argv)
    ingest_weather(
        args.input,
        args.output,
        variables=args.variables,
        engine=args.engine,
        latitude_dim=args.latitude_dim,
        longitude_dim=args.longitude_dim,
        time_dim=args.time_dim,
    )


if __name__ == "__main__":
    main()
