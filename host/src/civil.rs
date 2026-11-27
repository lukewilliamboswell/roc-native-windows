use crate::Result;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Date,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    Unset,
    Date { year: u16, month: u16, day: u16 },
    Time { hour: u16, minute: u16 },
}

const FIRST_YEAR: u16 = 1753;
const LAST_YEAR: u16 = 9999;

impl Value {
    fn date(year: u16, month: u16, day: u16) -> Result<Self> {
        if !(FIRST_YEAR..=LAST_YEAR).contains(&year) {
            return Err(format!(
                "date year must be from {FIRST_YEAR} through {LAST_YEAR}"
            ));
        }
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let limit = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => 28 + u16::from(leap),
            _ => 0,
        };
        if day == 0 || day > limit {
            return Err("invalid Gregorian calendar date".into());
        }
        Ok(Self::Date { year, month, day })
    }

    fn time(hour: u16, minute: u16) -> Result<Self> {
        if hour > 23 || minute > 59 {
            return Err("invalid clock time".into());
        }
        Ok(Self::Time { hour, minute })
    }

    pub(crate) fn parse(kind: Kind, text: &str) -> Result<Self> {
        if text.is_empty() {
            return Ok(Self::Unset);
        }
        match (kind, text.as_bytes()) {
            (Kind::Date, [year @ .., b'-', m1, m2, b'-', d1, d2]) if year.len() == 4 => {
                Self::date(number(year)?, number(&[*m1, *m2])?, number(&[*d1, *d2])?)
            }
            (Kind::Time, [h1, h2, b':', m1, m2]) => {
                Self::time(number(&[*h1, *h2])?, number(&[*m1, *m2])?)
            }
            (Kind::Date, _) => Err("expected a YYYY-MM-DD date".into()),
            (Kind::Time, _) => Err("expected an HH:MM time".into()),
        }
    }
}

fn number(digits: &[u8]) -> Result<u16> {
    digits.iter().try_fold(0, |n, &digit| match digit {
        b'0'..=b'9' => Ok(n * 10 + u16::from(digit - b'0')),
        _ => Err("expected an ASCII digit".into()),
    })
}

impl std::fmt::Display for Value {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => Ok(()),
            Self::Date { year, month, day } => write!(out, "{year:04}-{month:02}-{day:02}"),
            Self::Time { hour, minute } => write!(out, "{hour:02}:{minute:02}"),
        }
    }
}

#[path = "civil_format.rs"]
mod format;

#[cfg(windows)]
pub(crate) mod native {
    use super::Kind;
    use super::format::CivilFormat;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Globalization::{
        GetLocaleInfoEx, LOCALE_S1159, LOCALE_S2359, LOCALE_SSHORTDATE, LOCALE_SSHORTTIME,
    };

    pub(crate) fn format(kind: Kind) -> CivilFormat {
        let pattern = match kind {
            Kind::Date => LOCALE_SSHORTDATE,
            Kind::Time => LOCALE_SSHORTTIME,
        };
        CivilFormat::new(
            kind,
            &locale_setting(pattern),
            [locale_setting(LOCALE_S1159), locale_setting(LOCALE_S2359)],
        )
    }

    fn locale_setting(kind: u32) -> String {
        // SAFETY: A null locale selects the current user. A zero-length query
        // returns the required size including the UTF-16 terminator.
        let count = unsafe { GetLocaleInfoEx(null(), kind, null_mut(), 0) };
        if count <= 1 {
            return String::new();
        }
        let mut text = vec![0; count as usize];
        // SAFETY: The writable buffer contains count UTF-16 units.
        let written = unsafe { GetLocaleInfoEx(null(), kind, text.as_mut_ptr(), count) };
        String::from_utf16_lossy(&text[..written.saturating_sub(1).max(0) as usize])
    }
}

#[cfg(windows)]
pub(crate) use format::CivilFormat;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_gregorian_range_and_calendar_days() {
        assert_eq!(
            Value::parse(Kind::Date, "1753-01-01").unwrap().to_string(),
            "1753-01-01"
        );
        assert!(Value::parse(Kind::Date, "9999-12-31").is_ok());
        assert!(
            Value::parse(Kind::Date, "1752-12-31")
                .unwrap_err()
                .contains("year")
        );
        assert!(Value::parse(Kind::Date, "2000-02-29").is_ok());
        for rejected in [
            "1900-02-29",
            "2026-04-31",
            "2026-00-01",
            "2026-10-00",
            "2026-1-20",
        ] {
            assert!(Value::parse(Kind::Date, rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn clock_reads_hours_and_minutes_and_allows_unset() {
        assert_eq!(Value::parse(Kind::Time, "").unwrap(), Value::Unset);
        assert_eq!(
            Value::parse(Kind::Time, "00:00").unwrap().to_string(),
            "00:00"
        );
        assert!(Value::parse(Kind::Time, "23:59").is_ok());
        for rejected in ["24:00", "12:60", "12:00:00", "1:00", "01:x4"] {
            assert!(Value::parse(Kind::Time, rejected).is_err(), "{rejected}");
        }
    }
}
