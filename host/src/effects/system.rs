//! Private port and crypto wire adapters for Windows system services.

use crate::Result;
use crate::regional::{self, FormatError, TimeZone};
use crate::wire::{Reader, put_str};

const MAX_FORMATTED: usize = 64;

pub(super) fn port_body<'a>(reader: &mut Reader<'a>) -> Result<(Option<&'a str>, u32)> {
    let value = reader.string()?;
    let blobs = reader.u32()?;
    for _ in 0..blobs {
        let length = reader.u32()? as usize;
        reader.raw(length)?;
    }
    let files = reader.u32()? as usize;
    reader.raw(
        files
            .checked_mul(24)
            .ok_or("native system attachment size overflow")?,
    )?;
    Ok(((blobs == 0 && files == 0).then_some(value), reader.u32()?))
}

pub(super) fn crypto_count(reader: &mut Reader<'_>) -> Result<Option<u64>> {
    let byte_arg = |reader: &mut Reader<'_>| -> Result<()> {
        let length = reader.u32()? as usize;
        reader.raw(length)?;
        Ok(())
    };
    match reader.u8()? {
        1 => return reader.u64().map(Some),
        2 => {
            reader.raw(5)?;
            reader.string()?;
            byte_arg(reader)?;
        }
        3 => {
            reader.u64()?;
            byte_arg(reader)?;
            byte_arg(reader)?;
        }
        4 => {
            reader.u64()?;
            byte_arg(reader)?;
            byte_arg(reader)?;
            byte_arg(reader)?;
        }
        5 => {
            reader.u8()?;
            byte_arg(reader)?;
        }
        _ => return Err("unknown crypto request".into()),
    }
    Ok(None)
}

pub(super) fn secure_random(count: u64) -> Vec<u8> {
    if count > regional::MAX_RANDOM {
        return vec![30, 2];
    }
    let mut bytes = vec![0; count as usize + 1];
    if regional::random(&mut bytes[1..]) {
        bytes[0] = 28;
        bytes
    } else {
        vec![30, 6]
    }
}

pub(super) fn monotonic(code: u8, ticks: u64) -> Vec<u8> {
    answer(format!(r#"{{"code":{code},"ticks":"{}"}}"#, ticks))
}

pub(super) fn regional_settings() -> Vec<u8> {
    let locale = regional::format_locale()
        .or_else(|_| regional::locale_setting(windows_sys::Win32::Globalization::LOCALE_SNAME));
    match (locale, TimeZone::read()) {
        (Ok(locale), Some(zone)) => regional_answer(0, &locale, zone.name().as_deref()),
        _ => regional_answer(1, "", None),
    }
}

pub(super) fn regional_answer(code: u8, locale: &str, zone: Option<&str>) -> Vec<u8> {
    answer(format!(
        r#"{{"code":{code},"locale":{},"zone":{}}}"#,
        json_str(locale),
        json_str(zone.unwrap_or_default())
    ))
}

pub(super) struct FormatRequest {
    pub(super) instants: Vec<i64>,
    pub(super) long: bool,
    pub(super) local: bool,
}

impl FormatRequest {
    pub(super) fn read(input: &str) -> Result<Option<Self>> {
        let (date, rest) = input
            .strip_prefix(r#"{"date":""#)
            .and_then(|text| text.split_once(r#"","instants":["#))
            .ok_or("invalid timestamp request fields")?;
        let (instants, zone) = rest
            .strip_suffix(r#""}"#)
            .and_then(|text| text.split_once(r#"],"zone":""#))
            .ok_or("invalid timestamp request fields")?;
        let long = match date {
            "LongDate" => true,
            "ShortDate" => false,
            _ => return Err("unknown date format".into()),
        };
        let local = match zone {
            "Local" => true,
            "Utc" => false,
            _ => return Err("unknown time zone mode".into()),
        };
        let mut values = Vec::new();
        if !instants.is_empty() {
            for text in instants.split(',') {
                if values.len() >= MAX_FORMATTED {
                    return Ok(None);
                }
                let at = text
                    .strip_prefix('"')
                    .and_then(|text| text.strip_suffix('"'))
                    .and_then(|text| text.parse().ok())
                    .ok_or("invalid signed timestamp")?;
                values.push(at);
            }
        }
        Ok(Some(Self {
            instants: values,
            long,
            local,
        }))
    }

    pub(super) fn format(self) -> Vec<u8> {
        match regional::format_timestamps(&self.instants, self.long, self.local) {
            Ok(parts) => formatted_answer(0, &parts),
            Err(FormatError::Unavailable) => formatted_answer(1, &[]),
            Err(FormatError::OutsideRange) => formatted_answer(4, &[]),
        }
    }
}

pub(super) fn formatted_answer(code: u8, parts: &[(String, String)]) -> Vec<u8> {
    let items = parts
        .iter()
        .map(|(date, time)| format!(r#"{{"date":{},"time":{}}}"#, json_str(date), json_str(time)))
        .collect::<Vec<_>>()
        .join(",");
    answer(format!(r#"{{"code":{code},"parts":[{items}]}}"#))
}

fn answer(json: String) -> Vec<u8> {
    let mut bytes = vec![14];
    put_str(&mut bytes, &json);
    bytes.extend([0; 8]); // no binary or file attachments
    bytes
}

fn json_str(text: &str) -> String {
    use std::fmt::Write;
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\0'..='\x1f' => {
                write!(out, "\\u{:04x}", c as u32).unwrap();
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_size_and_result_are_framed() {
        assert_eq!(secure_random(0), [28]);
        assert_eq!(secure_random(65_537), [30, 2]);
        let first = secure_random(65_536);
        let second = secure_random(65_536);
        assert_eq!(first.len(), 65_537);
        assert_eq!((first[0], second[0]), (28, 28));
        assert_ne!(first, second);
    }

    #[test]
    fn formatting_preserves_signed_values_and_escapes_regional_text() {
        let format =
            FormatRequest::read(r#"{"date":"LongDate","instants":["-1","123"],"zone":"Utc"}"#)
                .unwrap()
                .unwrap();
        assert!(format.long && !format.local);
        assert_eq!(format.instants, [-1, 123]);
        assert_eq!(json_str("a\\\"é\n"), "\"a\\\\\\\"é\\u000a\"");
    }
}
