use crate::civil::{FIRST_YEAR, LAST_YEAR, Value, days_in_month};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Date {
    pub(crate) year: u16,
    pub(crate) month: u16,
    pub(crate) day: u16,
}

impl Date {
    pub(crate) fn from_value(value: Value) -> Option<Self> {
        match value {
            Value::Date { year, month, day } => Some(Self { year, month, day }),
            _ => None,
        }
    }

    pub(crate) fn value(self) -> Value {
        Value::Date {
            year: self.year,
            month: self.month,
            day: self.day,
        }
    }

    pub(crate) fn month_days(self) -> u16 {
        days_in_month(self.year, self.month)
    }

    pub(crate) fn first_weekday(self) -> u16 {
        // 1 January of Gregorian year 1 was a Monday. Count completed days.
        let years = u32::from(self.year) - 1;
        let completed_years = 365 * years + years / 4 - years / 100 + years / 400;
        let completed_months: u32 = (1..self.month)
            .map(|month| u32::from(Self { month, ..self }.month_days()))
            .sum();
        ((completed_years + completed_months) % 7) as u16
    }

    pub(crate) fn change_month(self, offset: i32) -> Option<Self> {
        let month = i32::from(self.year) * 12 + i32::from(self.month) - 1 + offset;
        if !(i32::from(FIRST_YEAR) * 12..=i32::from(LAST_YEAR) * 12 + 11).contains(&month) {
            return None;
        }
        let next = Self {
            year: (month / 12) as u16,
            month: (month % 12 + 1) as u16,
            ..self
        };
        Some(Self {
            day: self.day.min(next.month_days()),
            ..next
        })
    }

    pub(crate) fn change_day(self, offset: i32) -> Self {
        let day = i32::from(self.day) + offset;
        if day < 1 {
            if let Some(previous) = self.change_month(-1) {
                return Self {
                    day: (i32::from(previous.month_days()) + day) as u16,
                    ..previous
                };
            }
            return Self { day: 1, ..self };
        }
        let length = i32::from(self.month_days());
        if day > length {
            if let Some(next) = self.change_month(1) {
                return Self {
                    day: (day - length) as u16,
                    ..next
                };
            }
            return Self {
                day: length as u16,
                ..self
            };
        }
        Self {
            day: day as u16,
            ..self
        }
    }
}

#[cfg(windows)]
#[path = "civil_calendar_native.rs"]
pub(crate) mod native;

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: u16, month: u16, day: u16) -> Date {
        Date::from_value(Value::date(year, month, day).unwrap()).unwrap()
    }

    #[test]
    fn day_and_week_navigation_cross_months_and_respect_leap_days() {
        assert_eq!(date(2028, 2, 1).first_weekday(), 1);
        assert_eq!(date(2028, 2, 28).change_day(1), date(2028, 2, 29));
        assert_eq!(date(2028, 2, 29).change_day(1), date(2028, 3, 1));
        assert_eq!(date(2028, 3, 1).change_day(-7), date(2028, 2, 23));
        assert_eq!(date(1900, 1, 31).change_month(1), Some(date(1900, 2, 28)));
        assert_eq!(date(2028, 1, 31).change_month(1), Some(date(2028, 2, 29)));
    }

    #[test]
    fn navigation_stops_at_both_supported_bounds() {
        let first = date(1753, 1, 1);
        let last = date(9999, 12, 31);
        assert_eq!(first.change_month(-1), None);
        assert_eq!(last.change_month(1), None);
        assert_eq!(first.change_day(-7).value(), first.value());
        assert_eq!(last.change_day(7).value(), last.value());
    }
}
