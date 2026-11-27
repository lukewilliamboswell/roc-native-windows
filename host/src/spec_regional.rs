//! Deterministic regional facts used by native scenarios.

use crate::{
    Result,
    regional::{Snapshot, Zone},
};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(crate) struct Regional {
    pub(crate) locale: String,
    pub(crate) name: Option<String>,
    languages: Vec<String>,
    zone: Option<Zone>,
    formats: BTreeMap<(i64, bool, bool), (String, String)>,
}

impl Default for Regional {
    fn default() -> Self {
        Self {
            locale: "en-US".into(),
            name: None,
            languages: vec!["en-US".into()],
            zone: None,
            formats: BTreeMap::new(),
        }
    }
}

impl Regional {
    pub(crate) fn read(text: &str) -> Result<Self> {
        let mut result = Self {
            languages: Vec::new(),
            ..Self::default()
        };
        let mut bounds = None;
        let mut offsets = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (kind, data) = line
                .split_once(' ')
                .ok_or("expected a regional fact and its value")?;
            match kind {
                "locale" if !data.is_empty() && data.len() <= 85 => result.locale = data.into(),
                "zone" if !data.is_empty() && data.len() <= 128 => result.name = Some(data.into()),
                "language"
                    if !data.is_empty() && data.len() <= 64 && result.languages.len() < 16 =>
                {
                    result.languages.push(data.into())
                }
                "span" if bounds.is_none() => {
                    let (from, until) = data
                        .split_once(' ')
                        .ok_or("expected start and end in the zone span")?;
                    bounds = Some((timestamp(from)?, timestamp(until)?));
                }
                "offset" if offsets.len() < 128 => {
                    let (at, minutes) = data
                        .split_once(' ')
                        .ok_or("expected timestamp and minutes in the offset")?;
                    let minutes: i32 = minutes.parse().map_err(|_| "invalid offset minutes")?;
                    if !(-1440..=1440).contains(&minutes) {
                        return Err("offset minutes are out of range".into());
                    }
                    offsets.push((timestamp(at)?, minutes));
                }
                "formatted" => {
                    let [mode, style, at, date, time] = data
                        .split('|')
                        .collect::<Vec<_>>()
                        .try_into()
                        .map_err(|_| "expected mode|style|timestamp|date|time")?;
                    let local = match mode {
                        "local" => true,
                        "utc" => false,
                        _ => return Err("timestamp mode is utc or local".into()),
                    };
                    let long = match style {
                        "long" => true,
                        "short" => false,
                        _ => return Err("timestamp style is short or long".into()),
                    };
                    if result
                        .formats
                        .insert((timestamp(at)?, local, long), (date.into(), time.into()))
                        .is_some()
                    {
                        return Err("duplicate formatted timestamp".into());
                    }
                }
                _ => return Err(format!("invalid regional fact {kind:?}")),
            }
        }
        if result.languages.is_empty() {
            result.languages.push("en-US".into());
        }
        if let Some((from, until)) = bounds {
            if from > until
                || offsets.first().is_none_or(|&(at, _)| at != from)
                || offsets.last().is_some_and(|&(at, _)| at > until)
                || offsets
                    .windows(2)
                    .any(|pair| pair[0].0 >= pair[1].0 || pair[0].1 == pair[1].1)
            {
                return Err("invalid inclusive zone span or offset order".into());
            }
            result.zone = Some(Zone {
                from,
                until,
                offsets,
            });
        } else if !offsets.is_empty() {
            return Err("zone offsets need a span".into());
        }
        Ok(result)
    }

    pub(crate) fn snapshot(&self, now: i64, days: u16) -> Option<Snapshot> {
        let mut facts = Snapshot::simulated(now, days);
        facts.languages.clone_from(&self.languages);
        if let Some(zone) = &self.zone {
            // Intersect the fixture's known coverage with the requested span.
            let from = facts.zone.from.max(zone.from);
            let until = facts.zone.until.min(zone.until);
            if from <= now && now <= until {
                let before = zone.offsets.partition_point(|&(at, _)| at <= from);
                let end = zone.offsets.partition_point(|&(at, _)| at <= until);
                let mut offsets = vec![(from, zone.offsets[before - 1].1)];
                offsets.extend_from_slice(&zone.offsets[before..end]);
                facts.zone = Zone {
                    from,
                    until,
                    offsets,
                };
            } else {
                return None;
            }
        }
        Some(facts)
    }

    pub(crate) fn format(
        &self,
        instants: &[i64],
        local: bool,
        long: bool,
    ) -> std::result::Result<Vec<(String, String)>, u8> {
        instants
            .iter()
            .map(|&at| {
                if !(-11_644_473_600_000..=253_402_300_799_999).contains(&at) {
                    return Err(4);
                }
                self.formats.get(&(at, local, long)).cloned().ok_or(1)
            })
            .collect()
    }
}

fn timestamp(value: &str) -> Result<i64> {
    value
        .parse()
        .map_err(|_| "invalid signed Unix milliseconds".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_clips_to_requested_window_and_formats_exact_parts() {
        let fixture = Regional::read("locale en-GB\nlanguage fr-FR\nlanguage en-GB\nzone GMT Standard Time\nspan -100 100\noffset -100 0\noffset 5 60\nformatted utc|short|0|01/01/1970|00:00:00").unwrap();
        let snapshot = fixture.snapshot(5, 0).unwrap();
        assert_eq!(snapshot.languages, ["fr-FR", "en-GB"]);
        assert_eq!(snapshot.zone.offsets, [(5, 60)]);
        assert_eq!(
            fixture.format(&[0], false, false),
            Ok(vec![("01/01/1970".into(), "00:00:00".into())])
        );
        assert_eq!(fixture.format(&[i64::MAX], false, false), Err(4));
        assert!(Regional::read("span 0 100\noffset 1 0").is_err());
    }
}
