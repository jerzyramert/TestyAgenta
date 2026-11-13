use std::fs::{self, File};
use std::path::{Path, PathBuf};

use memmap2::{Mmap, MmapOptions};
use serde_json::Value;

use crate::error::DomainError;

#[derive(Clone, Copy)]
enum NumericType {
    Float32(bool),
    Float64(bool),
    Int64(bool),
}

impl NumericType {
    fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "<f4" | "|f4" => Ok(Self::Float32(true)),
            ">f4" => Ok(Self::Float32(false)),
            "<f8" | "|f8" => Ok(Self::Float64(true)),
            ">f8" => Ok(Self::Float64(false)),
            "<i8" | "|i8" => Ok(Self::Int64(true)),
            ">i8" => Ok(Self::Int64(false)),
            _ => Err(DomainError::InvalidZarr(format!(
                "unsupported numeric dtype {value}"
            ))),
        }
    }

    fn bytes(self) -> usize {
        match self {
            Self::Float32(_) => 4,
            Self::Float64(_) | Self::Int64(_) => 8,
        }
    }

    fn read_f64(self, bytes: &[u8]) -> Result<f64, DomainError> {
        match self {
            Self::Float32(little_endian) => {
                let raw: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| DomainError::InvalidZarr("truncated float chunk".to_owned()))?;
                let value = if little_endian {
                    f32::from_le_bytes(raw)
                } else {
                    f32::from_be_bytes(raw)
                };
                Ok(f64::from(value))
            }
            Self::Float64(little_endian) => {
                let raw: [u8; 8] = bytes
                    .try_into()
                    .map_err(|_| DomainError::InvalidZarr("truncated float chunk".to_owned()))?;
                Ok(if little_endian {
                    f64::from_le_bytes(raw)
                } else {
                    f64::from_be_bytes(raw)
                })
            }
            Self::Int64(little_endian) => {
                let raw: [u8; 8] = bytes
                    .try_into()
                    .map_err(|_| DomainError::InvalidZarr("truncated integer chunk".to_owned()))?;
                let value = if little_endian {
                    i64::from_le_bytes(raw)
                } else {
                    i64::from_be_bytes(raw)
                };
                Ok(value as f64)
            }
        }
    }

    fn read_i64(self, bytes: &[u8]) -> Result<i64, DomainError> {
        match self {
            Self::Int64(little_endian) => {
                let raw: [u8; 8] = bytes[..8]
                    .try_into()
                    .map_err(|_| DomainError::InvalidZarr("truncated integer chunk".to_owned()))?;
                Ok(if little_endian {
                    i64::from_le_bytes(raw)
                } else {
                    i64::from_be_bytes(raw)
                })
            }
            _ => Err(DomainError::InvalidZarr(
                "time coordinates must use an int64 Zarr array".to_owned(),
            )),
        }
    }
}

pub struct MmapZarrArray {
    path: PathBuf,
    shape: Vec<usize>,
    chunks: Vec<usize>,
    chunk_grid: Vec<usize>,
    dtype: NumericType,
    fill_value: f64,
    mappings: Vec<Option<Mmap>>,
}

impl MmapZarrArray {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DomainError> {
        let path = path.as_ref().to_path_buf();
        let metadata_path = path.join(".zarray");
        let metadata_bytes = fs::read(&metadata_path).map_err(|error| {
            DomainError::InvalidZarr(format!("{}: {error}", metadata_path.display()))
        })?;
        let metadata: Value = serde_json::from_slice(&metadata_bytes)
            .map_err(|error| DomainError::InvalidZarr(error.to_string()))?;

        if metadata["zarr_format"].as_u64() != Some(2) {
            return Err(DomainError::InvalidZarr(
                "only Zarr v2 arrays are supported".to_owned(),
            ));
        }
        if !metadata["compressor"].is_null()
            || !(metadata["filters"].is_null()
                || metadata["filters"].as_array().is_some_and(Vec::is_empty))
        {
            return Err(DomainError::InvalidZarr(
                "zero-copy access requires uncompressed chunks without filters".to_owned(),
            ));
        }
        if metadata["order"].as_str() != Some("C") {
            return Err(DomainError::InvalidZarr(
                "only C-order Zarr arrays are supported".to_owned(),
            ));
        }

        let shape = parse_dimensions(&metadata["shape"], "shape")?;
        let chunks = parse_dimensions(&metadata["chunks"], "chunks")?;
        if shape.is_empty() || shape.len() != chunks.len() {
            return Err(DomainError::InvalidZarr(
                "shape and chunks must have the same non-zero rank".to_owned(),
            ));
        }
        if chunks.contains(&0) || shape.contains(&0) {
            return Err(DomainError::InvalidZarr(
                "zero-sized dimensions and chunks are unsupported".to_owned(),
            ));
        }

        let dtype_value = metadata["dtype"]
            .as_str()
            .ok_or_else(|| DomainError::InvalidZarr("missing dtype".to_owned()))?;
        let dtype = NumericType::parse(dtype_value)?;
        let separator = match metadata["dimension_separator"].as_str().unwrap_or(".") {
            "." => '.',
            "/" => '/',
            other => {
                return Err(DomainError::InvalidZarr(format!(
                    "unsupported dimension separator {other}"
                )));
            }
        };
        let fill_value = parse_fill_value(&metadata["fill_value"])?;

        let chunk_grid: Vec<usize> = shape
            .iter()
            .zip(&chunks)
            .map(|(size, chunk)| size.div_ceil(*chunk))
            .collect();
        let chunk_count = chunk_grid.iter().product();
        let mut mappings = Vec::with_capacity(chunk_count);
        for linear_index in 0..chunk_count {
            let chunk_indices = unravel_index(linear_index, &chunk_grid);
            let chunk_path = chunk_path(&path, &chunk_indices, separator);
            if !chunk_path.exists() {
                mappings.push(None);
                continue;
            }

            let file = File::open(&chunk_path).map_err(|error| {
                DomainError::InvalidZarr(format!("{}: {error}", chunk_path.display()))
            })?;
            let expected_bytes = chunks.iter().product::<usize>() * dtype.bytes();
            if file
                .metadata()
                .map_err(|error| DomainError::InvalidZarr(error.to_string()))?
                .len()
                < expected_bytes as u64
            {
                return Err(DomainError::InvalidZarr(format!(
                    "truncated chunk {}",
                    chunk_path.display()
                )));
            }
            // The read-only mapping remains owned by this array for the lifetime of every query.
            let mapping = unsafe { MmapOptions::new().map(&file) }
                .map_err(|error| DomainError::InvalidZarr(error.to_string()))?;
            mappings.push(Some(mapping));
        }

        Ok(Self {
            path,
            shape,
            chunks,
            chunk_grid,
            dtype,
            fill_value,
            mappings,
        })
    }

    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    pub fn dimensions(&self) -> Result<Vec<String>, DomainError> {
        let metadata_path = self.path.join(".zattrs");
        let attributes: Value =
            serde_json::from_slice(&fs::read(&metadata_path).map_err(|error| {
                DomainError::InvalidZarr(format!("{}: {error}", metadata_path.display()))
            })?)
            .map_err(|error| DomainError::InvalidZarr(error.to_string()))?;
        let dimensions = attributes["_ARRAY_DIMENSIONS"]
            .as_array()
            .ok_or_else(|| DomainError::InvalidZarr("missing _ARRAY_DIMENSIONS".to_owned()))?;
        dimensions
            .iter()
            .map(|dimension| {
                dimension.as_str().map(str::to_owned).ok_or_else(|| {
                    DomainError::InvalidZarr("invalid _ARRAY_DIMENSIONS entry".to_owned())
                })
            })
            .collect()
    }

    pub fn units(&self) -> Result<String, DomainError> {
        let metadata_path = self.path.join(".zattrs");
        let attributes: Value =
            serde_json::from_slice(&fs::read(&metadata_path).map_err(|error| {
                DomainError::InvalidZarr(format!("{}: {error}", metadata_path.display()))
            })?)
            .map_err(|error| DomainError::InvalidZarr(error.to_string()))?;
        attributes["units"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                DomainError::InvalidZarr(format!(
                    "missing units attribute for {}",
                    self.path.display()
                ))
            })
    }

    pub fn value_f64(&self, indices: &[usize]) -> Result<f64, DomainError> {
        let (mapping, offset) = self.locate_value(indices)?;
        let Some(mapping) = mapping else {
            return Ok(self.fill_value);
        };
        let end = offset + self.dtype.bytes();
        if end > mapping.len() {
            return Err(DomainError::InvalidZarr(
                "chunk index points beyond mapped data".to_owned(),
            ));
        }
        self.dtype.read_f64(&mapping[offset..end])
    }

    pub fn value_i64(&self, indices: &[usize]) -> Result<i64, DomainError> {
        let (mapping, offset) = self.locate_value(indices)?;
        let Some(mapping) = mapping else {
            return Err(DomainError::InvalidZarr(
                "time coordinate chunk is missing".to_owned(),
            ));
        };
        let end = offset + self.dtype.bytes();
        if end > mapping.len() {
            return Err(DomainError::InvalidZarr(
                "chunk index points beyond mapped data".to_owned(),
            ));
        }
        self.dtype.read_i64(&mapping[offset..end])
    }

    fn locate_value(&self, indices: &[usize]) -> Result<(Option<&Mmap>, usize), DomainError> {
        if indices.len() != self.shape.len()
            || indices
                .iter()
                .zip(&self.shape)
                .any(|(index, size)| index >= size)
        {
            return Err(DomainError::InvalidZarr(
                "array index is outside the Zarr shape".to_owned(),
            ));
        }

        let mut mapping_index = 0;
        let mut local_index = 0;
        for axis in 0..indices.len() {
            mapping_index =
                mapping_index * self.chunk_grid[axis] + indices[axis] / self.chunks[axis];
            local_index = local_index * self.chunks[axis] + indices[axis] % self.chunks[axis];
        }
        let offset = local_index * self.dtype.bytes();
        let mapping = self
            .mappings
            .get(mapping_index)
            .ok_or_else(|| DomainError::InvalidZarr("chunk grid index is invalid".to_owned()))?
            .as_ref();
        Ok((mapping, offset))
    }
}

fn parse_dimensions(value: &Value, name: &str) -> Result<Vec<usize>, DomainError> {
    value
        .as_array()
        .ok_or_else(|| DomainError::InvalidZarr(format!("missing {name}")))?
        .iter()
        .map(|dimension| {
            dimension
                .as_u64()
                .and_then(|size| usize::try_from(size).ok())
                .ok_or_else(|| DomainError::InvalidZarr(format!("invalid {name} entry")))
        })
        .collect()
}

fn parse_fill_value(value: &Value) -> Result<f64, DomainError> {
    match value {
        Value::Null => Ok(0.0),
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| DomainError::InvalidZarr("invalid fill value".to_owned())),
        Value::String(value) => match value.as_str() {
            "NaN" => Ok(f64::NAN),
            "Infinity" => Ok(f64::INFINITY),
            "-Infinity" => Ok(f64::NEG_INFINITY),
            _ => value
                .parse::<f64>()
                .map_err(|error| DomainError::InvalidZarr(error.to_string())),
        },
        _ => Err(DomainError::InvalidZarr("invalid fill value".to_owned())),
    }
}

fn chunk_path(array_path: &Path, indices: &[usize], separator: char) -> PathBuf {
    let key = indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(&separator.to_string());
    array_path.join(key)
}

fn unravel_index(mut index: usize, dimensions: &[usize]) -> Vec<usize> {
    let mut result = vec![0; dimensions.len()];
    for axis in (0..dimensions.len()).rev() {
        result[axis] = index % dimensions[axis];
        index /= dimensions[axis];
    }
    result
}
