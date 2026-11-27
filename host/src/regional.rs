//! Bounded Windows clock, locale, and time-zone facts.

use crate::Result;

const DAY_MS: i64 = 86_400_000;
const MAX_TRANSITIONS: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct Zone {
    pub(crate) from: i64,
    pub(crate) until: i64,
    pub(crate) offsets: Vec<(i64, i32)>,
}

impl Zone {
    pub(crate) fn constant(now: i64, days: u16, minutes: i32) -> Self {
        let (from, until) = reach(now, days);
        Self {
            from,
            until,
            offsets: vec![(from, minutes)],
        }
    }
}

pub(crate) struct Snapshot {
    pub(crate) now: i64,
    pub(crate) seed: [u8; 16],
    pub(crate) languages: Vec<String>,
    pub(crate) zone: Zone,
}

impl Snapshot {
    pub(crate) fn simulated(now: i64, days: u16) -> Self {
        Self {
            now,
            seed: [1; 16],
            languages: vec!["en-US".into()],
            zone: Zone::constant(now, days, 0),
        }
    }
}

fn reach(now: i64, days: u16) -> (i64, i64) {
    let reach = i64::from(days) * DAY_MS;
    (now.saturating_sub(reach), now.saturating_add(reach))
}

fn collect_zone(
    now: i64,
    days: u16,
    candidates: impl IntoIterator<Item = i64>,
    mut offset_at: impl FnMut(i64) -> Option<i32>,
) -> Option<Zone> {
    let (mut from, mut until) = reach(now, days);
    let mut candidates = candidates
        .into_iter()
        .filter(|at| *at > from && *at <= until)
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.dedup();
    let mut offsets = vec![(from, offset_at(from)?)];
    for at in candidates {
        let before = offset_at(at - 1)?;
        let after = offset_at(at)?;
        if before != offsets.last()?.1 {
            return None;
        }
        if before != after {
            offsets.push((at, after));
        }
    }
    let error = offset_at(until)? != offsets.last()?.1
        || offset_at(now)? != offsets[offsets.partition_point(|(at, _)| *at <= now) - 1].1;
    if error {
        return None;
    }
    if offsets.len() > MAX_TRANSITIONS {
        let active = offsets.partition_point(|(at, _)| *at <= now) - 1;
        let start = active
            .saturating_sub(MAX_TRANSITIONS / 2)
            .min(offsets.len() - MAX_TRANSITIONS);
        let stop = start + MAX_TRANSITIONS;
        from = offsets[start].0;
        if let Some(&(next, _)) = offsets.get(stop) {
            until = next - 1;
        }
        offsets = offsets[start..stop].to_vec();
    }
    Some(Zone {
        from,
        until,
        offsets,
    })
}

pub(crate) fn unix_ms(time: std::time::SystemTime) -> Result<i64> {
    use std::time::UNIX_EPOCH;
    let millis = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as i128,
        Err(error) => -(error.duration().as_nanos().div_ceil(1_000_000) as i128),
    };
    i64::try_from(millis).map_err(|_| "wall clock outside signed Unix milliseconds".into())
}

fn languages(buffer: &[u16]) -> Option<Vec<String>> {
    if !buffer.ends_with(&[0, 0]) {
        return None;
    }
    let tags = buffer.split(|&c| c == 0).take_while(|tag| !tag.is_empty());
    let tags = tags
        .filter_map(|tag| String::from_utf16(tag).ok().filter(|s| s.len() <= 64))
        .take(16)
        .collect::<Vec<_>>();
    (!tags.is_empty()).then_some(tags)
}

#[cfg(windows)]
pub(crate) use native::*;

#[cfg(windows)]
mod native {
    use super::*;
    use std::ptr::null;
    use std::sync::OnceLock;
    use std::time::SystemTime;
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::Globalization::*;
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Time::*;
    use windows_sys::core::{s, w};

    const FILETIME_UNIX_EPOCH: i128 = 116_444_736_000_000_000;
    pub(crate) const MAX_RANDOM: u64 = 65_536;

    #[derive(Clone, Copy)]
    pub(crate) struct TimeZone(DYNAMIC_TIME_ZONE_INFORMATION);

    #[derive(Clone, Copy)]
    pub(crate) enum FormatError {
        Unavailable,
        OutsideRange,
    }

    pub(crate) fn random(bytes: &mut [u8]) -> bool {
        if bytes.is_empty() {
            return true;
        }
        // SAFETY: The system-preferred generator accepts a null algorithm
        // handle and fills the supplied writable buffer.
        unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            ) == 0
        }
    }

    pub(crate) fn snapshot(days: u16, boot: bool, live_entropy: bool) -> Result<Snapshot> {
        let now = unix_ms(SystemTime::now())?;
        let mut seed = [1; 16];
        if live_entropy && !random(&mut seed) {
            return Err("could not obtain random seed from Windows".into());
        }
        let zone = match TimeZone::read().and_then(|zone| zone.table(now, days)) {
            Some(zone) => zone,
            None if boot => {
                eprintln!(
                    "roc-native-windows: could not obtain a Windows time-zone offset; using UTC"
                );
                Zone::constant(now, days, 0)
            }
            None => return Err("could not obtain a Windows time-zone offset".into()),
        };
        Ok(Snapshot {
            now,
            seed,
            languages: preferred_languages().unwrap_or_else(|| vec!["en-US".into()]),
            zone,
        })
    }

    fn preferred_languages() -> Option<Vec<String>> {
        let (mut count, mut size) = (0, 0);
        // SAFETY: First call queries the MULTI_SZ size, second fills it.
        if unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                std::ptr::null_mut(),
                &mut size,
            )
        } == 0
            || !(2..=65_536).contains(&size)
        {
            return None;
        }
        let mut buffer = vec![0; size as usize];
        if unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                buffer.as_mut_ptr(),
                &mut size,
            )
        } == 0
        {
            return None;
        }
        languages(buffer.get(..size as usize)?)
    }

    pub(crate) fn locale_setting(kind: u32) -> Result<String> {
        // SAFETY: A null locale selects user preferences, including overrides.
        formatted(|buffer, len| unsafe { GetLocaleInfoEx(null(), kind, buffer, len) })
            .ok_or_else(|| "Windows regional setting unavailable".into())
    }

    pub(crate) fn format_locale() -> Result<String> {
        // SAFETY: Supply a writable Windows locale-name buffer.
        let mut name = [0; 85];
        let len = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
        if len <= 1 {
            return Err("Windows regional locale unavailable".into());
        }
        String::from_utf16(&name[..len as usize - 1]).map_err(|error| error.to_string())
    }

    pub(crate) fn format_timestamps(
        instants: &[i64],
        long: bool,
        local: bool,
    ) -> std::result::Result<Vec<(String, String)>, FormatError> {
        if instants.is_empty() {
            return Ok(Vec::new());
        }
        let zone = if local {
            Some(TimeZone::read().ok_or(FormatError::Unavailable)?)
        } else {
            None
        };
        let style = if long { DATE_LONGDATE } else { DATE_SHORTDATE };
        instants
            .iter()
            .map(|&at| {
                let utc = calendar(at).ok_or(FormatError::OutsideRange)?;
                let time = match zone {
                    Some(zone) => zone.project(&utc).ok_or(FormatError::OutsideRange)?,
                    None => utc,
                };
                if time.wYear > 9999 {
                    return Err(FormatError::OutsideRange);
                }
                // SAFETY: All input pointers remain valid and the closure fills a sized buffer.
                let date = formatted(|out, len| unsafe {
                    GetDateFormatEx(null(), style, &time, null(), out, len, null())
                })
                .ok_or(FormatError::Unavailable)?;
                let time = formatted(|out, len| unsafe {
                    GetTimeFormatEx(null(), 0, &time, null(), out, len)
                })
                .ok_or(FormatError::Unavailable)?;
                Ok((date, time))
            })
            .collect()
    }

    fn formatted(mut call: impl FnMut(*mut u16, i32) -> i32) -> Option<String> {
        let len = call(std::ptr::null_mut(), 0);
        if !(1..=1024).contains(&len) {
            return None;
        }
        let mut buf = vec![0; len as usize];
        let count = call(buf.as_mut_ptr(), len);
        if !(1..=len).contains(&count) {
            return None;
        }
        String::from_utf16(&buf[..count as usize - 1]).ok()
    }

    fn calendar(at: i64) -> Option<SYSTEMTIME> {
        let ticks = u64::try_from(i128::from(at) * 10_000 + FILETIME_UNIX_EPOCH).ok()?;
        let file = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let mut result = SYSTEMTIME::default();
        // SAFETY: Both pointers reference their respective initialized structures.
        (unsafe { FileTimeToSystemTime(&file, &mut result) } != 0).then_some(result)
    }

    fn millis(local: &SYSTEMTIME) -> Option<i64> {
        let mut file = FILETIME::default();
        // SAFETY: Both pointers reference their respective initialized structures.
        if unsafe { SystemTimeToFileTime(local, &mut file) } == 0 {
            return None;
        }
        let ticks = (u64::from(file.dwHighDateTime) << 32) | u64::from(file.dwLowDateTime);
        i64::try_from((i128::from(ticks) - FILETIME_UNIX_EPOCH) / 10_000).ok()
    }

    impl TimeZone {
        pub(crate) fn read() -> Option<Self> {
            let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
            // SAFETY: Windows writes into a complete dynamic zone record.
            (unsafe { GetDynamicTimeZoneInformation(&mut info) } != TIME_ZONE_ID_INVALID)
                .then_some(Self(info))
        }

        pub(crate) fn name(self) -> Option<String> {
            let name = self.0.TimeZoneKeyName.split(|c| *c == 0).next()?;
            (!name.is_empty()).then(|| String::from_utf16_lossy(name))
        }

        fn project(self, utc: &SYSTEMTIME) -> Option<SYSTEMTIME> {
            if self.0.DynamicDaylightTimeDisabled {
                let offset = self.0.Bias.checked_add(self.0.StandardBias)?;
                return calendar(millis(utc)?.checked_sub(i64::from(offset) * 60_000)?);
            }
            if requires_compatible_projection() {
                return self.project_year_rule(utc);
            }
            let mut local = SYSTEMTIME::default();
            // SAFETY: The captured zone and immutable UTC input outlive this call.
            (unsafe { SystemTimeToTzSpecificLocalTimeEx(&self.0, utc, &mut local) } != 0)
                .then_some(local)
        }

        fn year_rule(self, year: u16) -> Option<TIME_ZONE_INFORMATION> {
            let mut info = TIME_ZONE_INFORMATION::default();
            // SAFETY: The captured dynamic zone is valid and the yearly output is writable.
            (unsafe { GetTimeZoneInformationForYear(year, &self.0, &mut info) } != 0)
                .then_some(info)
        }

        fn project_year_rule(self, utc: &SYSTEMTIME) -> Option<SYSTEMTIME> {
            let year = utc.wYear;
            // The appropriate rule is indexed by local year, which can differ
            // from the UTC year at the boundary.
            for year in [year, year.saturating_sub(1), year.saturating_add(1)] {
                let Some(rule) = self.year_rule(year) else {
                    continue;
                };
                let mut local = SYSTEMTIME::default();
                // SAFETY: The sampled year rule, UTC input, and output are valid.
                if unsafe { SystemTimeToTzSpecificLocalTime(&rule, utc, &mut local) } != 0
                    && local.wYear == year
                {
                    return Some(local);
                }
            }
            None
        }

        fn offset_at(self, at: i64) -> Option<i32> {
            let local = self.project(&calendar(at)?)?;
            let offset = millis(&local)?.checked_sub(at)? / 60_000;
            let offset = i32::try_from(offset).ok()?;
            (-1440..=1440).contains(&offset).then_some(offset)
        }

        fn table(self, now: i64, days: u16) -> Option<Zone> {
            let offset = self.offset_at(now)?;
            let full = || {
                let (from, until) = reach(now, days);
                let mut candidates = Vec::new();
                if !self.0.DynamicDaylightTimeDisabled {
                    let from = self
                        .project(&calendar(from)?)?
                        .wYear
                        .saturating_sub(1)
                        .max(1601);
                    let until = self.project(&calendar(until)?)?.wYear.saturating_add(1);
                    for year in from..=until {
                        let info = self.year_rule(year)?;
                        let standard = info.Bias.checked_add(info.StandardBias)?;
                        let daylight = info.Bias.checked_add(info.DaylightBias)?;
                        for (rule, before) in
                            [(info.DaylightDate, standard), (info.StandardDate, daylight)]
                        {
                            if rule.wMonth != 0 {
                                candidates
                                    .push(transition(year, rule)? + i64::from(before) * 60_000);
                            }
                        }
                        // Include UTC, old-local, and new-local year boundaries.
                        // Offsets from the preceding year can also apply on January 1.
                        let next = millis(&SYSTEMTIME {
                            wYear: year.checked_add(1)?,
                            wMonth: 1,
                            wDay: 1,
                            ..SYSTEMTIME::default()
                        })?;
                        let first = millis(&SYSTEMTIME {
                            wYear: year,
                            wMonth: 1,
                            wDay: 1,
                            ..SYSTEMTIME::default()
                        })?;
                        for boundary in [first, next] {
                            for bias in [0, standard, daylight] {
                                candidates.push(boundary + i64::from(bias) * 60_000);
                            }
                        }
                    }
                }
                collect_zone(now, days, candidates, |at| self.offset_at(at))
            };
            full().or_else(|| Some(Zone::constant(now, 0, offset)))
        }
    }

    fn requires_compatible_projection() -> bool {
        static WINE: OnceLock<bool> = OnceLock::new();
        *WINE.get_or_init(|| {
            // Older Wine versions export Ex as an aborting stub. A Wine
            // marker identifies that compatibility environment without
            // invoking the stub; native Windows always uses Ex.
            unsafe {
                let ntdll = GetModuleHandleW(w!("ntdll.dll"));
                !ntdll.is_null() && GetProcAddress(ntdll, s!("wine_get_version")).is_some()
            }
        })
    }

    fn transition(year: u16, mut rule: SYSTEMTIME) -> Option<i64> {
        if rule.wYear != 0 {
            return millis(&rule);
        }
        let weekday = rule.wDayOfWeek;
        let week = rule.wDay;
        if weekday > 6 || !(1..=5).contains(&week) {
            return None;
        }
        rule.wYear = year;
        rule.wDay = 1;
        let first = millis(&rule)?;
        let weekday_of_first = calendar(first)?.wDayOfWeek;
        rule.wDay = 1 + (weekday + 7 - weekday_of_first) % 7 + (week - 1) * 7;
        if week == 5 && millis(&rule).is_none() {
            rule.wDay -= 7;
        }
        millis(&rule)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn instant(year: u16, month: u16, day: u16) -> i64 {
            millis(&SYSTEMTIME {
                wYear: year,
                wMonth: month,
                wDay: day,
                ..SYSTEMTIME::default()
            })
            .unwrap()
        }

        fn named_zone(name: &str) -> TimeZone {
            for index in 0..1024 {
                let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
                // SAFETY: Windows initializes the complete time-zone record on success.
                if unsafe { EnumDynamicTimeZoneInformation(index, &mut info) } != 0 {
                    break;
                }
                let zone = TimeZone(info);
                if zone.name().as_deref() == Some(name) {
                    return zone;
                }
            }
            panic!("Windows did not enumerate {name:?}");
        }

        #[test]
        fn windows_rules_match_native_offsets_at_span_year_and_dst_boundaries() {
            let now = instant(2026, 7, 1);
            for (name, changes, shift) in [
                ("UTC", 0, 0),
                ("Pacific Standard Time", 4, 60),
                ("Lord Howe Standard Time", 4, 30),
            ] {
                let native = named_zone(name);
                let table = native.table(now, 366).unwrap();
                assert_eq!((table.from, table.until), reach(now, 366), "{name}");
                assert_eq!(table.offsets.len(), changes + 1, "{name}");
                assert_eq!(table.offsets.first().unwrap().0, table.from);
                for pair in table.offsets.windows(2) {
                    let (at, after) = pair[1];
                    assert_eq!(native.offset_at(at - 1), Some(pair[0].1), "{name}");
                    assert_eq!(native.offset_at(at), Some(after), "{name}");
                    assert_eq!((after - pair[0].1).abs(), shift, "{name}");
                }
                for at in [
                    table.from,
                    table.until,
                    instant(2026, 1, 1),
                    instant(2027, 1, 1),
                ] {
                    let index = table.offsets.partition_point(|&(first, _)| first <= at) - 1;
                    assert_eq!(native.offset_at(at), Some(table.offsets[index].1), "{name}");
                }
                let mut disabled = native;
                disabled.0.DynamicDaylightTimeDisabled = true;
                assert_eq!(disabled.table(now, 366).unwrap().offsets.len(), 1, "{name}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_is_signed_and_language_order_is_preserved() {
        use std::time::{Duration, UNIX_EPOCH};
        assert_eq!(unix_ms(UNIX_EPOCH - Duration::from_nanos(100)), Ok(-1));
        assert_eq!(unix_ms(UNIX_EPOCH + Duration::from_millis(17)), Ok(17));
        let input = "fr-CA\0en-CA\0\0".encode_utf16().collect::<Vec<_>>();
        assert_eq!(languages(&input).unwrap(), ["fr-CA", "en-CA"]);
        assert!(languages(&input[..input.len() - 1]).is_none());
    }

    #[test]
    fn southern_half_hour_shift_keeps_exact_edges_and_endpoints() {
        let events = [(-DAY_MS, 630), (DAY_MS, 660)];
        let sample = |at| {
            Some(
                events
                    .iter()
                    .rev()
                    .find(|(first, _)| *first <= at)
                    .map_or(660, |(_, bias)| *bias),
            )
        };
        let zone = collect_zone(0, 2, [DAY_MS, -DAY_MS, 0, DAY_MS], sample).unwrap();
        assert_eq!((zone.from, zone.until), (-2 * DAY_MS, 2 * DAY_MS));
        assert_eq!(
            zone.offsets,
            [(-2 * DAY_MS, 660), (-DAY_MS, 630), (DAY_MS, 660)]
        );
        assert!(collect_zone(0, 2, [DAY_MS], sample).is_none());
        assert_eq!(
            collect_zone(0, 2, [], |_| Some(330)).unwrap().offsets,
            [(-2 * DAY_MS, 330)]
        );
    }

    #[test]
    fn a_shortened_zone_keeps_now_and_an_inclusive_end() {
        let candidates = -200..=200;
        let zone = collect_zone(0, 1, candidates, |at| {
            Some((at.clamp(-201, 200) % 2 != 0) as i32)
        })
        .unwrap();
        assert_eq!(zone.offsets.len(), MAX_TRANSITIONS);
        assert!(zone.from < 0 && zone.until > 0);
        assert_eq!(zone.offsets[0].0, zone.from);
        assert_eq!(zone.offsets.last().unwrap().0, zone.until);
    }
}
