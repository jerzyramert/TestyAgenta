import argparse
import math
import os
from collections.abc import Iterable, Iterator
from pathlib import Path

import geopandas
import psycopg
from pyproj import CRS, Transformer
from shapely import make_valid
from shapely import transform as transform_geometry
from shapely.geometry import MultiPolygon, Polygon
from shapely.geometry.base import BaseGeometry
from shapely.ops import unary_union

COASTLINE_BUFFER_M = 50.0
DEFAULT_SIMPLIFY_TOLERANCE_M = 100.0


def _polygon_parts(geometry: BaseGeometry) -> list[Polygon]:
    if isinstance(geometry, Polygon):
        return [geometry]
    if isinstance(geometry, MultiPolygon):
        return list(geometry.geoms)
    if hasattr(geometry, "geoms"):
        return [part for child in geometry.geoms for part in _polygon_parts(child)]
    return []


def prepare_coastline(
    geometry: BaseGeometry,
    simplify_tolerance_m: float = DEFAULT_SIMPLIFY_TOLERANCE_M,
) -> MultiPolygon:
    if not math.isfinite(simplify_tolerance_m) or simplify_tolerance_m < 0:
        raise ValueError("simplify_tolerance_m must be finite and non-negative")

    polygon_parts = _polygon_parts(make_valid(geometry))
    if not polygon_parts:
        raise ValueError("coastline feature must contain polygonal geometry")

    geographic_geometry = make_valid(unary_union(polygon_parts))
    centroid = geographic_geometry.centroid
    local_crs = CRS.from_proj4(
        f"+proj=aeqd +lat_0={centroid.y} +lon_0={centroid.x} "
        "+datum=WGS84 +units=m +no_defs"
    )
    to_local = Transformer.from_crs("EPSG:4326", local_crs, always_xy=True)
    to_geographic = Transformer.from_crs(local_crs, "EPSG:4326", always_xy=True)
    buffered = transform_geometry(
        geographic_geometry,
        to_local.transform,
        interleaved=False,
    ).buffer(COASTLINE_BUFFER_M)
    if simplify_tolerance_m > 0:
        buffered = buffered.simplify(simplify_tolerance_m, preserve_topology=True)

    result_parts = _polygon_parts(
        make_valid(
            transform_geometry(
                buffered,
                to_geographic.transform,
                interleaved=False,
            )
        )
    )
    if not result_parts:
        raise ValueError("buffered coastline did not produce polygonal geometry")
    return MultiPolygon(result_parts)


def iter_coastline_features(
    source_path: Path, layer: str | None = None
) -> Iterator[tuple[str, BaseGeometry]]:
    offset = 0
    batch_size = 1_000
    while True:
        features = geopandas.read_file(
            source_path,
            layer=layer,
            rows=slice(offset, offset + batch_size),
            engine="pyogrio",
        )
        if features.empty:
            break
        if features.crs is None:
            raise ValueError(f"source has no CRS: {source_path}")
        features = features.to_crs("EPSG:4326")
        for feature_id, geometry in zip(features.index, features.geometry, strict=True):
            if geometry is None:
                raise ValueError(f"feature {feature_id} has no geometry")
            yield str(feature_id), geometry
        offset += len(features)


def bulk_insert_coastlines(
    connection: psycopg.Connection[object],
    features: Iterable[tuple[str, BaseGeometry]],
    simplify_tolerance_m: float = DEFAULT_SIMPLIFY_TOLERANCE_M,
) -> int:
    rows = [
        (source, bytes(prepare_coastline(geometry, simplify_tolerance_m).wkb))
        for source, geometry in features
    ]
    if not rows:
        return 0

    with connection.cursor() as cursor:
        cursor.executemany(
            """
            INSERT INTO coastlines (source, geom)
            VALUES (%s, ST_Multi(ST_SetSRID(ST_GeomFromWKB(%s), 4326)))
            """,
            rows,
        )
    return len(rows)


def main() -> None:
    parser = argparse.ArgumentParser(description="Load buffered S-57/GSHHG coastlines into PostGIS.")
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--layer")
    parser.add_argument(
        "--simplify-tolerance-m",
        type=float,
        default=DEFAULT_SIMPLIFY_TOLERANCE_M,
    )
    parser.add_argument("--dsn", default=os.environ.get("DATABASE_URL"))
    args = parser.parse_args()
    if not args.dsn:
        parser.error("--dsn or DATABASE_URL is required")

    with psycopg.connect(args.dsn) as connection:
        count = bulk_insert_coastlines(
            connection,
            iter_coastline_features(args.source, args.layer),
            args.simplify_tolerance_m,
        )
    print(f"Inserted {count} coastline features")


if __name__ == "__main__":
    main()
