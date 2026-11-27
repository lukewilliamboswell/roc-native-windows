//! Decode a selected file while its original file capability is retained.

use super::files::{self, Entry};
use crate::Result;
use crate::raster::{MAX_ENCODED_BYTES, Pixels, decode_bytes, supports};
use crate::wire::Reader;
use std::sync::Arc;

pub(super) struct Request {
    file: Option<Entry>,
    start: u64,
    len: u64,
    pub(super) timeout: u32,
}

impl Request {
    pub(super) fn read(reader: &mut Reader<'_>) -> Result<Self> {
        if reader.string()? != r#"{"$file":0}"# || reader.u32()? != 0 || reader.u32()? != 1 {
            return Err("decode-image expects exactly one file attachment".into());
        }
        let file = files::resolve(reader.u64()?).ok();
        let start = reader.u64()?;
        let len = reader.u64()?;
        let timeout = reader.u32()?;
        Ok(Self {
            file,
            start,
            len,
            timeout,
        })
    }

    pub(super) fn decode(self) -> std::result::Result<Arc<Pixels>, u8> {
        if self.len > MAX_ENCODED_BYTES as u64 {
            return Err(3);
        }
        let file = self.file.ok_or(1)?;
        let data = file.read_range(self.start, self.len).map_err(|_| 1)?;
        if !supports(&data) {
            return Err(2);
        }
        decode_bytes(&data).map_err(|error| {
            if error.contains("dimensions exceed") {
                3
            } else {
                4
            }
        })
    }
}

pub(super) fn answer(code: u8, with_file: bool) -> Vec<u8> {
    let files = if with_file { r#"[{"$file":0}]"# } else { "[]" };
    let json = format!(r#"{{"code":{code},"files":{files}}}"#);
    let mut record = vec![14];
    super::put_string(&mut record, &json);
    record.extend(0u32.to_le_bytes());
    record.extend(u32::from(with_file).to_le_bytes());
    record
}
