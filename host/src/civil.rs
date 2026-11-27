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

#[cfg(windows)]
pub(crate) mod native {
    use super::{FIRST_YEAR, Kind, LAST_YEAR, Value};
    use crate::Result;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, SYSTEMTIME};
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SSHORTTIME};
    use windows_sys::Win32::UI::Controls::{
        DTM_GETSYSTEMTIME, DTM_SETFORMATW, DTM_SETRANGE, DTM_SETSYSTEMTIME, GDT_NONE, GDT_VALID,
        GDTR_MAX, GDTR_MIN, NMDATETIMECHANGE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    pub(crate) fn initialize(hwnd: HWND, kind: Kind) -> Result<()> {
        match kind {
            Kind::Date => {
                let range = [
                    system_time(Value::Date {
                        year: FIRST_YEAR,
                        month: 1,
                        day: 1,
                    }),
                    SYSTEMTIME {
                        wHour: 23,
                        wMinute: 59,
                        wSecond: 59,
                        wMilliseconds: 999,
                        ..system_time(Value::Date {
                            year: LAST_YEAR,
                            month: 12,
                            day: 31,
                        })
                    },
                ];
                // SAFETY: The two valid SYSTEMTIMEs remain live for this synchronous call.
                if unsafe {
                    SendMessageW(
                        hwnd,
                        DTM_SETRANGE,
                        (GDTR_MIN | GDTR_MAX) as usize,
                        range.as_ptr() as LPARAM,
                    )
                } == 0
                {
                    return Err("could not set the native date picker's range".into());
                }
            }
            Kind::Time => {
                let mut format = [0u16; 128];
                // SAFETY: A null locale asks for the current user's locale. The buffer is writable.
                let len = unsafe {
                    GetLocaleInfoEx(
                        std::ptr::null(),
                        LOCALE_SSHORTTIME,
                        format.as_mut_ptr(),
                        format.len() as i32,
                    )
                };
                // SAFETY: A successful query leaves a terminated format string in the live buffer.
                if len == 0
                    || unsafe { SendMessageW(hwnd, DTM_SETFORMATW, 0, format.as_ptr() as LPARAM) }
                        == 0
                {
                    return Err("could not set the native time picker's short-time format".into());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn read(hwnd: HWND, kind: Kind) -> Result<Value> {
        let mut fields = SYSTEMTIME::default();
        // SAFETY: The window is a live picker and fields remains writable for the call.
        let flag = unsafe {
            SendMessageW(
                hwnd,
                DTM_GETSYSTEMTIME,
                0,
                &mut fields as *mut SYSTEMTIME as LPARAM,
            )
        };
        match flag as u32 {
            GDT_NONE => Ok(Value::Unset),
            GDT_VALID => decode(kind, fields),
            _ => Err("could not read the native calendar or clock value".into()),
        }
    }

    pub(crate) fn restore(hwnd: HWND, kind: Kind, value: Value) -> Result<()> {
        if read(hwnd, kind)? == value {
            return Ok(());
        }
        let fields = system_time(value);
        let flag = if value == Value::Unset {
            GDT_NONE
        } else {
            GDT_VALID
        };
        // DTM_SETSYSTEMTIME ignores the fields for GDT_NONE on Windows.
        // A live pointer also works with Win32 implementations that require one.
        // SAFETY: The SYSTEMTIME remains live for the synchronous message.
        let applied = unsafe {
            SendMessageW(
                hwnd,
                DTM_SETSYSTEMTIME,
                flag as usize,
                &fields as *const SYSTEMTIME as LPARAM,
            )
        };
        if applied == 0 || read(hwnd, kind)? != value {
            return Err("the native picker rejected the controlled value".into());
        }
        Ok(())
    }

    /// The caller checked the WM_NOTIFY code and resolved a live date or time picker.
    pub(crate) unsafe fn notification(lparam: LPARAM, kind: Kind) -> Result<Value> {
        let event = lparam as *const NMDATETIMECHANGE;
        // SAFETY: Win32 owns the message payload for the duration of WM_NOTIFY.
        // Only GDT_VALID guarantees that the time fields are initialized.
        match unsafe { (*event).dwFlags } {
            GDT_NONE => Ok(Value::Unset),
            GDT_VALID => decode(kind, unsafe { (*event).st }),
            _ => Err("unexpected state from a native calendar or clock picker".into()),
        }
    }

    fn decode(kind: Kind, fields: SYSTEMTIME) -> Result<Value> {
        match kind {
            Kind::Date => Value::date(fields.wYear, fields.wMonth, fields.wDay),
            Kind::Time => Value::time(fields.wHour, fields.wMinute),
        }
    }

    fn system_time(value: Value) -> SYSTEMTIME {
        match value {
            Value::Date { year, month, day } => SYSTEMTIME {
                wYear: year,
                wMonth: month,
                wDay: day,
                ..SYSTEMTIME::default()
            },
            Value::Time { hour, minute } => SYSTEMTIME {
                // Win32 needs a full SYSTEMTIME. These date fields are never exposed.
                wYear: 2000,
                wMonth: 1,
                wDay: 1,
                wHour: hour,
                wMinute: minute,
                ..SYSTEMTIME::default()
            },
            Value::Unset => SYSTEMTIME::default(),
        }
    }
}

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
