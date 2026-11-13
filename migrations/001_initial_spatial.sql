CREATE EXTENSION IF NOT EXISTS postgis;

CREATE TABLE coastlines (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source text NOT NULL,
    geom geometry(MultiPolygon, 4326) NOT NULL,
    active_from timestamptz NOT NULL DEFAULT '-infinity',
    active_to timestamptz,
    CHECK (active_to IS NULL OR active_to > active_from)
);

CREATE INDEX coastlines_geom_gist ON coastlines USING GIST (geom);
CREATE INDEX coastlines_active_period ON coastlines (active_from, active_to);

CREATE TABLE bathymetry (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    depth_m double precision NOT NULL CHECK (depth_m >= 0),
    geom geometry(MultiPolygon, 4326) NOT NULL,
    active_from timestamptz NOT NULL DEFAULT '-infinity',
    active_to timestamptz,
    CHECK (active_to IS NULL OR active_to > active_from)
);

CREATE INDEX bathymetry_geom_gist ON bathymetry USING GIST (geom);
CREATE INDEX bathymetry_active_period ON bathymetry (active_from, active_to);

CREATE TABLE no_go_zones (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    geom geometry(MultiPolygon, 4326) NOT NULL,
    active_from timestamptz NOT NULL,
    active_to timestamptz,
    CHECK (active_to IS NULL OR active_to > active_from)
);

CREATE INDEX no_go_zones_geom_gist ON no_go_zones USING GIST (geom);
CREATE INDEX no_go_zones_active_period ON no_go_zones (active_from, active_to);
