//! A bounded reader for the shader-model-5 DXBC metadata used by the view.

use crate::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::str;

pub(crate) const MAX_BYTES: u64 = 1024 * 1024;
const MAX_PARAMETERS: usize = 256;

#[derive(Debug)]
pub(crate) struct Reflection {
    pub(crate) width: usize,
    pub(crate) textures: [bool; 2],
    offsets: BTreeMap<String, usize>,
}

impl Reflection {
    pub(crate) fn read(bytes: &[u8]) -> Result<Self> {
        let chunks = chunks(bytes)?;
        let code = chunks
            .get(b"SHDR")
            .or_else(|| chunks.get(b"SHEX"))
            .ok_or("shader DXBC has no shader program")?;
        if word(code, 0)? != 0x50 || word(code, 4)? != code.len() / 4 {
            return Err("shader must target ps_5_0".into());
        }
        verify_signature(chunks.get(b"ISGN").copied(), false)?;
        verify_signature(chunks.get(b"OSGN").copied(), true)?;
        let definitions = chunks
            .get(b"RDEF")
            .ok_or("shader DXBC has no RDEF reflection metadata")?;
        Self::read_definitions(definitions)
    }

    fn read_definitions(data: &[u8]) -> Result<Self> {
        if word(data, 16)? != 0xffff_0500 {
            return Err("shader reflection must target ps_5_0".into());
        }
        let buffers = records(data, 0, 24, 1)?;
        let bindings = records(data, 8, 32, 4)?;
        let mut slots = BTreeSet::new();
        let mut textures = [false; 2];
        let mut buffer_name = None;
        for binding in bindings {
            let name = name(data, word(binding, 0)?)?;
            let kind = word(binding, 4)?;
            let slot = word(binding, 20)?;
            let count = word(binding, 24)?;
            if count != 1 || !slots.insert((kind, slot)) {
                return Err(format!(
                    "unsupported binding array or duplicate slot for {name:?}"
                ));
            }
            match (kind, slot) {
                (0, 0) => buffer_name = Some(name),
                (2, 0..=1) if word(binding, 8)? == 5 && word(binding, 12)? == 4 => {
                    textures[slot] = true;
                }
                (3, 0) => {}
                _ => {
                    return Err(format!(
                        "unsupported shader binding {name:?}; use b0, t0/t1, and s0"
                    ));
                }
            }
        }
        let mut reflection = Self {
            width: 0,
            textures,
            offsets: BTreeMap::new(),
        };
        let Some(buffer) = buffers.into_iter().next() else {
            if buffer_name.is_some() {
                return Err("shader constant buffer metadata is inconsistent".into());
            }
            return Ok(reflection);
        };
        if Some(name(data, word(buffer, 0)?)?) != buffer_name || word(buffer, 20)? != 0 {
            return Err("shader constant buffer must be bound at b0".into());
        }
        let width = word(buffer, 12)?;
        if width == 0 || !width.is_multiple_of(16) || width > 4096 {
            return Err(
                "shader constant buffer must be at most 4096 bytes and aligned to 16".into(),
            );
        }
        reflection.width = width;
        let mut occupied = BTreeSet::new();
        let variables = table(data, word(buffer, 8)?, word(buffer, 4)?, 40, MAX_PARAMETERS)?;
        for variable in variables {
            let parameter = name(data, word(variable, 0)?)?;
            let offset = word(variable, 4)?;
            let ty = word(variable, 16)?;
            let kind = data.get(ty..).ok_or("invalid shader type offset")?;
            if word(variable, 8)? != 4
                || word(kind, 0)? != (3 << 16)
                || word(kind, 4)? != (1 | (1 << 16))
                || word(kind, 8)? != 0
            {
                return Err(format!(
                    "shader parameter {parameter:?} must be an F32 scalar"
                ));
            }
            if !offset.is_multiple_of(4)
                || offset > width - 4
                || !occupied.insert(offset)
                || reflection
                    .offsets
                    .insert(parameter.to_owned(), offset)
                    .is_some()
            {
                return Err(format!(
                    "invalid shader offset or duplicate parameter {parameter:?}"
                ));
            }
        }
        Ok(reflection)
    }

    pub(crate) fn constants(&self, parameters: &BTreeMap<String, f32>) -> Result<Vec<u8>> {
        let mut constants = vec![0u8; self.width];
        for (name, &offset) in &self.offsets {
            let value = parameters
                .get(name)
                .ok_or_else(|| format!("missing shader parameter {name:?}"))?;
            constants[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        if let Some(name) = parameters
            .keys()
            .find(|name| !self.offsets.contains_key(*name))
        {
            return Err(format!("unexpected shader parameter {name:?}"));
        }
        Ok(constants)
    }
}

pub(crate) fn parameters(text: &str) -> Result<BTreeMap<String, f32>> {
    if text.len() > 20_000 {
        return Err("shader parameter declaration is too large".into());
    }
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (name, raw) = line
            .split_once('=')
            .ok_or("a shader parameter needs a name and value")?;
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .enumerate()
                .all(|(at, c)| c == b'_' || c.is_ascii_alphabetic() || at > 0 && c.is_ascii_digit())
        {
            return Err(format!("invalid shader parameter name {name:?}"));
        }
        let value: f32 = raw
            .parse()
            .map_err(|_| format!("invalid F32 value for {name:?}"))?;
        if !value.is_finite() {
            return Err(format!("shader parameter {name:?} must be finite"));
        }
        if values.insert(name.to_owned(), value).is_some() {
            return Err(format!("duplicate shader parameter {name:?}"));
        }
        if values.len() > MAX_PARAMETERS {
            return Err("too many shader parameters".into());
        }
    }
    Ok(values)
}

fn chunks(bytes: &[u8]) -> Result<BTreeMap<[u8; 4], &[u8]>> {
    if bytes.get(..4) != Some(b"DXBC")
        || bytes.len() as u64 > MAX_BYTES
        || word(bytes, 24)? != bytes.len()
    {
        return Err("shader asset must contain one bounded DXBC program".into());
    }
    let count = word(bytes, 28)?;
    if count > 64 {
        return Err("too many shader DXBC chunks".into());
    }
    let floor = 32 + count * 4;
    let mut map = BTreeMap::new();
    let mut ranges = Vec::new();
    for index in 0..count {
        let offset = word(bytes, 32 + index * 4)?;
        if offset < floor || offset > bytes.len().saturating_sub(8) || !offset.is_multiple_of(4) {
            return Err("invalid shader chunk offset".into());
        }
        let tag: [u8; 4] = bytes[offset..offset + 4].try_into().unwrap();
        let start = offset + 8;
        let length = word(bytes, offset + 4)?;
        let end = start
            .checked_add(length)
            .ok_or("invalid shader chunk length")?;
        let chunk = bytes.get(start..end).ok_or("truncated shader chunk")?;
        if ranges.iter().any(|&(a, b)| a < end && offset < b) || map.insert(tag, chunk).is_some() {
            return Err("duplicate or overlapping shader chunks".into());
        }
        ranges.push((offset, end));
    }
    Ok(map)
}

fn verify_signature(data: Option<&[u8]>, output: bool) -> Result<()> {
    let data = data.ok_or("shader has no input or output signature")?;
    let elements = table(data, 8, word(data, 0)?, 24, 2)?;
    let mut seen = BTreeSet::new();
    for entry in elements {
        let semantic = name(data, word(entry, 0)?)?.to_ascii_uppercase();
        let index = word(entry, 4)?;
        let mask = word(entry, 20)? & 0x0f;
        let valid = if output {
            semantic == "SV_TARGET" && mask == 15
        } else {
            semantic == "TEXCOORD" && mask == 3 && word(entry, 16)? == 0
        };
        if !valid || index != 0 || word(entry, 12)? != 3 || !seen.insert(semantic) {
            return Err("shader input must be TEXCOORD0 float2 at register 0, and output must be SV_Target0 float4".into());
        }
    }
    if output && seen.len() != 1 {
        return Err("shader must write one SV_Target0 float4".into());
    }
    Ok(())
}

fn records(data: &[u8], at: usize, width: usize, limit: usize) -> Result<Vec<&[u8]>> {
    table(data, word(data, at + 4)?, word(data, at)?, width, limit)
}

fn table(
    data: &[u8],
    start: usize,
    count: usize,
    width: usize,
    limit: usize,
) -> Result<Vec<&[u8]>> {
    if count > limit {
        return Err("shader reflection table exceeds supported bounds".into());
    }
    let end = start
        .checked_add(count * width)
        .ok_or("shader reflection offset overflow")?;
    Ok(data
        .get(start..end)
        .ok_or("shader reflection record outside the asset")?
        .chunks_exact(width)
        .collect())
}

fn word(data: &[u8], at: usize) -> Result<usize> {
    if at > data.len().saturating_sub(4) {
        return Err("truncated shader metadata".into());
    }
    let bytes: [u8; 4] = data
        .get(at..at + 4)
        .ok_or("truncated shader metadata")?
        .try_into()
        .unwrap();
    Ok(u32::from_le_bytes(bytes) as usize)
}

fn name(data: &[u8], at: usize) -> Result<&str> {
    let bytes = data.get(at..).ok_or("shader name outside the asset")?;
    let end = bytes
        .iter()
        .take(257)
        .position(|&byte| byte == 0)
        .ok_or("unterminated shader name")?;
    if end == 0 {
        return Err("shader reflection names must be nonempty".into());
    }
    str::from_utf8(&bytes[..end]).map_err(|_| "shader reflection name is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_named_constants_and_texture_slots_from_compiled_examples() {
        let contrast = Reflection::read(include_bytes!(
            "../../examples/files/assets/scan-contrast.cso"
        ))
        .unwrap();
        assert_eq!(contrast.textures, [true, false]);
        assert_eq!(
            contrast
                .constants(&parameters("contrast=1.5").unwrap())
                .unwrap()[..4],
            1.5f32.to_le_bytes()
        );
        assert!(
            contrast
                .constants(&BTreeMap::new())
                .unwrap_err()
                .contains("missing")
        );
        assert!(
            contrast
                .constants(&parameters("contrast=1.0\nunused=0").unwrap())
                .unwrap_err()
                .contains("unexpected")
        );
        let compare = Reflection::read(include_bytes!(
            "../../examples/files/assets/image-compare.cso"
        ))
        .unwrap();
        assert_eq!(compare.textures, [true, true]);
        assert!(
            compare
                .constants(&parameters("divider=0.25").unwrap())
                .is_ok()
        );
    }

    #[test]
    fn a_procedural_shader_has_no_texture_bindings() {
        let meter = Reflection::read(include_bytes!(
            "../../examples/workers/assets/stock-variance.cso"
        ))
        .unwrap();
        assert_eq!(meter.textures, [false, false]);
        assert!(
            meter
                .constants(&parameters("mismatch=0.25").unwrap())
                .is_ok()
        );
    }

    #[test]
    fn rejects_duplicate_non_finite_and_truncated_parameters() {
        assert!(parameters("contrast=1\ncontrast=2").is_err());
        assert!(parameters("contrast=NaN").is_err());
        assert!(parameters("contrast=inf").is_err());
        assert!(parameters("contrast=0.9\njunk").is_err());
        assert!(Reflection::read(&b"DXBC"[..]).is_err());
    }
}
