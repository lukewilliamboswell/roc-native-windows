use super::{Kind, Value};
use crate::Result;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Year,
    Month,
    Day,
    Hour,
    Hour12,
    Minute,
    Period,
}

#[derive(Clone)]
enum Part {
    Field(Field, usize),
    Literal(String),
}

pub(crate) struct CivilFormat {
    kind: Kind,
    parts: Vec<Part>,
    periods: [String; 2],
}

impl CivilFormat {
    pub(crate) fn new(kind: Kind, pattern: &str, periods: [String; 2]) -> Self {
        let parts = parse_pattern(kind, pattern).unwrap_or_else(|| {
            parse_pattern(
                kind,
                match kind {
                    Kind::Date => "yyyy-MM-dd",
                    Kind::Time => "HH:mm",
                },
            )
            .unwrap()
        });
        Self {
            kind,
            parts,
            periods,
        }
    }

    pub(crate) fn example(&self) -> String {
        self.format(match self.kind {
            Kind::Date => Value::Date {
                year: 2026,
                month: 11,
                day: 23,
            },
            Kind::Time => Value::Time {
                hour: 17,
                minute: 45,
            },
        })
    }

    pub(crate) fn format(&self, value: Value) -> String {
        if value == Value::Unset {
            return String::new();
        }
        let mut out = String::new();
        for part in &self.parts {
            match (part, value) {
                (Part::Literal(literal), _) => out.push_str(literal),
                (Part::Field(Field::Period, width), Value::Time { hour, .. }) => {
                    out.push_str(&self.period(hour >= 12, *width));
                }
                (Part::Field(field, width), _) => {
                    let number = match (field, value) {
                        (Field::Year, Value::Date { year, .. }) => year,
                        (Field::Month, Value::Date { month, .. }) => month,
                        (Field::Day, Value::Date { day, .. }) => day,
                        (Field::Hour, Value::Time { hour, .. }) => hour,
                        (Field::Hour12, Value::Time { hour, .. }) => (hour + 11) % 12 + 1,
                        (Field::Minute, Value::Time { minute, .. }) => minute,
                        _ => unreachable!("validated civil pattern field"),
                    };
                    use std::fmt::Write as _;
                    let _ = write!(out, "{number:0width$}");
                }
            }
        }
        out
    }

    pub(crate) fn parse(&self, draft: &str) -> Result<Value> {
        let draft = draft.trim();
        Value::parse(self.kind, draft).or_else(|_| self.parse_local(draft))
    }

    fn parse_local(&self, mut input: &str) -> Result<Value> {
        let format_error = || format!("Use the format {}.", self.example());
        let mut values = [0u16; 7];
        for part in &self.parts {
            match part {
                Part::Literal(literal) => {
                    input = input.strip_prefix(literal).ok_or_else(&format_error)?;
                }
                Part::Field(Field::Period, width) => {
                    let (rest, afternoon) = [false, true]
                        .into_iter()
                        .find_map(|pm| {
                            let period = self.period(pm, *width);
                            let prefix = input.get(..period.len())?;
                            (prefix.to_lowercase() == period.to_lowercase())
                                .then_some((&input[prefix.len()..], pm))
                        })
                        .ok_or_else(&format_error)?;
                    values[Field::Period as usize] = u16::from(afternoon);
                    input = rest;
                }
                Part::Field(field, _) => {
                    let year = *field == Field::Year;
                    let max = if year { 4 } else { 2 };
                    let count = input
                        .bytes()
                        .take(max)
                        .take_while(u8::is_ascii_digit)
                        .count();
                    if count == 0 || (year && count != 4) {
                        return Err(format_error());
                    }
                    let (digits, rest) = input.split_at(count);
                    values[*field as usize] = digits.parse().map_err(|_| format_error())?;
                    input = rest;
                }
            }
        }
        if !input.is_empty() {
            return Err(format_error());
        }

        let part = |field: Field| values[field as usize];
        match self.kind {
            Kind::Date => Value::date(part(Field::Year), part(Field::Month), part(Field::Day)),
            Kind::Time => {
                let twelve = part(Field::Hour12);
                let hour = if self
                    .parts
                    .iter()
                    .any(|part| matches!(part, Part::Field(Field::Hour12, _)))
                {
                    if !(1..=12).contains(&twelve) {
                        return Err(format_error());
                    }
                    twelve % 12 + 12 * part(Field::Period)
                } else {
                    part(Field::Hour)
                };
                Value::time(hour, part(Field::Minute))
            }
        }
    }

    fn period(&self, afternoon: bool, width: usize) -> String {
        let period = &self.periods[usize::from(afternoon)];
        let period = if period.is_empty() {
            if afternoon { "PM" } else { "AM" }
        } else {
            period
        };
        if width == 1 {
            period.chars().take(1).collect()
        } else {
            period.to_owned()
        }
    }
}

fn parse_pattern(kind: Kind, pattern: &str) -> Option<Vec<Part>> {
    let mut chars = pattern.chars().peekable();
    let mut parts = Vec::new();
    let mut seen = Vec::new();
    let mut quoted = false;
    while let Some(character) = chars.next() {
        if character == '\'' {
            quoted = !quoted;
        } else if character.is_ascii_alphabetic() && !quoted {
            let field = match (kind, character) {
                (Kind::Date, 'y') => Field::Year,
                (Kind::Date, 'M') => Field::Month,
                (Kind::Date, 'd') => Field::Day,
                (Kind::Time, 'h') => Field::Hour12,
                (Kind::Time, 'H') => Field::Hour,
                (Kind::Time, 'm') => Field::Minute,
                (Kind::Time, 't') => Field::Period,
                _ => return None,
            };
            if seen.contains(&field) {
                return None;
            }
            seen.push(field);
            let mut count = 1;
            while chars.next_if_eq(&character).is_some() {
                count += 1;
            }
            let width = if field == Field::Year {
                4
            } else {
                count.min(2)
            };
            parts.push(Part::Field(field, width));
        } else if let Some(Part::Literal(previous)) = parts.last_mut() {
            previous.push(character);
        } else {
            parts.push(Part::Literal(character.to_string()));
        }
    }
    let has = |field| seen.contains(&field);
    let complete = match kind {
        Kind::Date => seen.len() == 3,
        Kind::Time => {
            has(Field::Minute)
                && ((seen.len() == 2 && has(Field::Hour))
                    || (seen.len() == 3 && has(Field::Hour12) && has(Field::Period)))
        }
    };
    (complete && !quoted).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_different_locale_date_orders_and_iso() {
        for (pattern, valid, invalid) in [
            ("M/d/yyyy", "2/29/2000", "2/29/1900"),
            ("dd.MM.yyyy", "29.02.2000", "29.02.1900"),
            ("yyyy'年'M'月'd'日'", "2000年2月29日", "1900年2月29日"),
        ] {
            let format = CivilFormat::new(Kind::Date, pattern, Default::default());
            let date = Value::parse(Kind::Date, "2000-02-29").unwrap();
            assert_eq!(format.parse(valid), Ok(date));
            assert_eq!(format.parse("2000-02-29"), Ok(date));
            assert_eq!(format.parse(&format.format(date)), Ok(date));
            assert_eq!(format.parse(" "), Ok(Value::Unset));
            assert!(format.parse(invalid).is_err());
            assert!(format.parse("1/2/26").is_err());
        }
    }

    #[test]
    fn accepts_local_day_periods_and_iso_minute_precision() {
        let format = CivilFormat::new(Kind::Time, "h:mm tt", ["am".into(), "pm".into()]);
        for (text, hour) in [
            ("12:00 AM", 0),
            ("12:00 PM", 12),
            ("7:00 PM", 19),
            ("19:00", 19),
        ] {
            let value = Value::Time { hour, minute: 0 };
            assert_eq!(format.parse(text), Ok(value));
            assert_eq!(format.parse(&format.format(value)), Ok(value));
        }
        assert!(format.parse("13:00 PM").is_err());
        assert!(format.parse("17:00:00").is_err());
    }
}
