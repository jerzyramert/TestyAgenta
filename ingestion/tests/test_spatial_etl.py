import os

import pytest
from ingestion.spatial_etl import DEFAULT_SIMPLIFY_TOLERANCE_M, prepare_coastline
from shapely.geometry import Polygon


def test_prepare_coastline_repairs_buffers_and_preserves_valid_polygons() -> None:
    coastline = Polygon(
        [(0.0, 0.0), (0.001, 0.001), (0.0, 0.001), (0.001, 0.0), (0.0, 0.0)]
    )

    result = prepare_coastline(coastline)

    assert result.is_valid
    assert result.geom_type == "MultiPolygon"
    assert result.area > 0


def test_default_simplification_tolerance_is_100_meters() -> None:
    coastline = Polygon(
        [(0.0, 0.0), (0.001, 0.0), (0.001, 0.001), (0.0, 0.001)]
    )

    assert DEFAULT_SIMPLIFY_TOLERANCE_M == 100.0
    assert prepare_coastline(coastline).equals(
        prepare_coastline(coastline, simplify_tolerance_m=100.0)
    )


def test_prepare_coastline_rejects_negative_simplification_tolerance() -> None:
    coastline = Polygon([(0, 0), (1, 0), (1, 1), (0, 1)])

    with pytest.raises(ValueError, match="non-negative"):
        prepare_coastline(coastline, simplify_tolerance_m=-1)


@pytest.mark.skipif(
    not os.environ.get("POSTGIS_TEST_DSN"),
    reason="POSTGIS_TEST_DSN must point to a PostGIS test database",
)
def test_postgis_geometry_validity_and_gist_index() -> None:
    import psycopg

    with (
        psycopg.connect(os.environ["POSTGIS_TEST_DSN"]) as connection,
        connection.cursor() as cursor,
    ):
        cursor.execute("CREATE TEMP TABLE spatial_etl_test (geom geometry(MultiPolygon, 4326))")
        cursor.execute("CREATE INDEX spatial_etl_test_geom_gist ON spatial_etl_test USING GIST (geom)")
        geometry = prepare_coastline(
            Polygon([(0, 0), (0.01, 0), (0.01, 0.01), (0, 0.01)])
        )
        cursor.execute(
            "INSERT INTO spatial_etl_test VALUES (ST_SetSRID(ST_GeomFromWKB(%s), 4326))",
            (bytes(geometry.wkb),),
        )
        cursor.execute("SELECT ST_IsValid(geom) FROM spatial_etl_test")
        assert cursor.fetchone() == (True,)
        cursor.execute(
            """
            SELECT am.amname
            FROM pg_index idx
            JOIN pg_class index_class ON index_class.oid = idx.indexrelid
            JOIN pg_am am ON am.oid = index_class.relam
            WHERE index_class.relname = 'spatial_etl_test_geom_gist'
            """
        )
        assert cursor.fetchone() == ("gist",)
