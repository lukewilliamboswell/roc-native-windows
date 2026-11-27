//! Simulated effects, numbered in issuance order independently of engine IDs.

use crate::Result;

pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) body: String,
    pub(crate) timeout: u32,
}

pub(crate) enum Kind {
    Http(Request),
    Timer(u32),
    Interval(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Status {
    Pending,
    Answered,
    Cancelled,
}

struct Entry {
    id: u32,
    time: u64,
    kind: Kind,
    status: Status,
}

#[derive(Default)]
pub(crate) struct Simulated {
    pub(crate) now: i64,
    pub(crate) mono: u64,
    effects: Vec<Entry>,
    random: Vec<(u32, u64, Status)>,
    staged: Option<(String, Vec<u8>)>,
}

impl Simulated {
    pub(crate) fn hold(&mut self, id: u32, kind: Kind) {
        self.effects.push(Entry {
            id,
            time: self.mono,
            kind,
            status: Status::Pending,
        });
    }

    pub(crate) fn cancel(&mut self, id: u32) {
        if let Some((_, _, status)) = self
            .random
            .iter_mut()
            .rev()
            .find(|(candidate, _, status)| *candidate == id && *status == Status::Pending)
        {
            *status = Status::Cancelled;
        }
        if let Some(entry) = self
            .effects
            .iter_mut()
            .rev()
            .find(|entry| entry.id == id && entry.status == Status::Pending)
        {
            entry.status = Status::Cancelled;
        }
    }

    pub(crate) fn hold_random(&mut self, id: u32, count: u64) {
        self.random.push((id, count, Status::Pending));
    }

    pub(crate) fn answer_random(
        &mut self,
        number: usize,
        reply: std::result::Result<Vec<u8>, u8>,
    ) -> Result<(u32, Vec<u8>)> {
        let (id, count, status) = self
            .random
            .get_mut(index(number)?)
            .ok_or("unknown random request")?;
        if *status != Status::Pending {
            return Err(format!("random request #{number} is {status:?}"));
        }
        let bytes = match reply {
            Ok(bytes) if bytes.len() as u64 == *count => {
                let mut record = vec![28];
                record.extend(bytes);
                record
            }
            Ok(_) => return Err("fixture length does not match the random request".into()),
            Err(code) => vec![30, code],
        };
        *status = Status::Answered;
        Ok((*id, bytes))
    }

    pub(crate) fn random_cancelled(&self, number: usize) -> Result<bool> {
        self.random
            .get(index(number)?)
            .map(|(_, _, status)| *status == Status::Cancelled)
            .ok_or_else(|| "unknown random request".into())
    }

    pub(crate) fn stage(&mut self, url: &str, payload: Vec<u8>) -> Result<()> {
        if self.staged.is_some() {
            return Err("an HTTP answer is already staged".into());
        }
        self.staged = Some((url.to_owned(), payload));
        Ok(())
    }

    pub(crate) fn run(&mut self, number: usize) -> Result<(u32, Vec<u8>)> {
        let entry = self
            .effects
            .get_mut(index(number)?)
            .ok_or_else(|| format!("unknown effect #{number}"))?;
        if entry.status != Status::Pending {
            return Err(format!("effect #{number} is {:?}", entry.status));
        }
        let payload = match &entry.kind {
            Kind::Timer(_) => tick(self.now),
            Kind::Interval(_) => return Err("fire an interval with tick-interval".into()),
            Kind::Http(request) => {
                let (url, _) = self.staged.as_ref().ok_or("no HTTP answer is staged")?;
                if !url.is_empty() && url != &request.url {
                    return Err(format!(
                        "the staged answer is for {url:?}, expected {:?}",
                        request.url
                    ));
                }
                self.staged.take().unwrap().1
            }
        };
        entry.status = Status::Answered;
        Ok((entry.id, payload))
    }

    pub(crate) fn timers(&self, delay: u32, recurring: bool) -> Vec<usize> {
        self.effects
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let period = match (&entry.kind, recurring) {
                    (Kind::Timer(period), false) | (Kind::Interval(period), true) => *period,
                    _ => return None,
                };
                (entry.status == Status::Pending && delay == period).then_some(index + 1)
            })
            .collect()
    }

    pub(crate) fn tick_interval(&self, number: usize) -> Option<(u32, Vec<u8>)> {
        let entry = self.effects.get(number.checked_sub(1)?)?;
        (entry.status == Status::Pending && matches!(entry.kind, Kind::Interval(_)))
            .then(|| (entry.id, tick(self.now)))
    }

    pub(crate) fn advance(&mut self, duration: i64) -> Result<()> {
        if duration < 0 {
            return Err("time cannot move backwards".into());
        }
        let now = self
            .now
            .checked_add(duration)
            .ok_or("wall clock overflow")?;
        let mono = self
            .mono
            .checked_add(duration as u64)
            .ok_or("monotonic clock overflow")?;
        self.now = now;
        self.mono = mono;
        Ok(())
    }

    pub(crate) fn next_timeout(&mut self) -> Option<u32> {
        let entry = self.effects.iter_mut().find(|entry| {
            entry.status == Status::Pending
                && matches!(&entry.kind, Kind::Http(req) if req.timeout != 0 &&
                    entry.time.saturating_add(u64::from(req.timeout)) <= self.mono)
        })?;
        entry.status = Status::Answered;
        Some(entry.id)
    }

    pub(crate) fn pending(&self) -> usize {
        self.effects
            .iter()
            .filter(|entry| {
                entry.status == Status::Pending && !matches!(entry.kind, Kind::Interval(_))
            })
            .count()
    }

    pub(crate) fn cancelled(&self, number: usize) -> Result<bool> {
        Ok(self.entry(number)?.status == Status::Cancelled)
    }

    pub(crate) fn request(&self, number: usize) -> Result<&Request> {
        match &self.entry(number)?.kind {
            Kind::Http(request) => Ok(request),
            _ => Err(format!("effect #{number} is not HTTP")),
        }
    }

    fn entry(&self, number: usize) -> Result<&Entry> {
        self.effects
            .get(index(number)?)
            .ok_or_else(|| format!("unknown effect #{number}"))
    }
}

fn index(number: usize) -> Result<usize> {
    number
        .checked_sub(1)
        .ok_or_else(|| "effect numbers start at 1".into())
}

pub(crate) fn tick(now: i64) -> Vec<u8> {
    let mut payload = vec![5];
    payload.extend(now.to_le_bytes());
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_replies_use_request_identity_and_wall_jumps_do_not_advance_monotonic_time() {
        let mut sim = Simulated::default();
        sim.hold_random(7, 2);
        sim.cancel(7);
        sim.hold_random(8, 2);
        assert!(sim.random_cancelled(1).unwrap());
        assert!(sim.answer_random(1, Ok(vec![1, 2])).is_err());
        assert!(sim.answer_random(2, Ok(vec![1])).is_err());
        assert_eq!(
            sim.answer_random(2, Ok(vec![1, 2])).unwrap(),
            (8, vec![28, 1, 2])
        );
        sim.hold_random(9, 2);
        assert_eq!(sim.answer_random(3, Err(5)).unwrap(), (9, vec![30, 5]));
        sim.hold(
            10,
            Kind::Http(Request {
                method: "GET".into(),
                url: "https://example.test".into(),
                body: String::new(),
                timeout: 25,
            }),
        );
        sim.now = 200;
        assert_eq!(sim.next_timeout(), None);
        sim.advance(25).unwrap();
        assert_eq!(sim.next_timeout(), Some(10));
        assert_eq!((sim.now, sim.mono), (225, 25));
    }

    fn http(timeout: u32) -> Kind {
        Kind::Http(Request {
            method: "GET".into(),
            url: "/example".into(),
            body: "".into(),
            timeout,
        })
    }

    #[test]
    fn expires_in_issue_order_and_cancellation_blocks_later_delivery() {
        let mut sim = Simulated::default();
        sim.hold(19, http(3));
        sim.hold(13, http(3));
        sim.hold(12, http(3));
        sim.advance(2).unwrap();
        assert_eq!(sim.next_timeout(), None);
        sim.advance(1).unwrap();
        assert_eq!(sim.next_timeout(), Some(19));
        sim.cancel(13);
        assert!(sim.cancelled(2).unwrap());
        assert_eq!(sim.next_timeout(), Some(12));
        assert_eq!(sim.next_timeout(), None);
    }

    #[test]
    fn answers_staged_http_and_ticks_both_kinds_of_timer() {
        let mut sim = Simulated::default();
        sim.hold(1, http(0));
        sim.hold(2, Kind::Timer(10));
        sim.hold(3, Kind::Interval(10));
        sim.stage("/example", vec![1, 2, 3]).unwrap();
        assert!(sim.stage("", vec![]).is_err());
        let req = sim.request(1).unwrap();
        assert_eq!((&*req.method, &*req.body), ("GET", ""));
        assert_eq!(sim.run(1).unwrap(), (1, vec![1, 2, 3]));
        sim.advance(10).unwrap();
        assert_eq!(sim.timers(10, false), [2]);
        assert_eq!(sim.run(2).unwrap(), (2, tick(10)));
        assert!(sim.timers(10, false).is_empty());
        assert_eq!(sim.timers(10, true), [3]);
        assert_eq!(sim.tick_interval(3), Some((3, tick(10))));
        assert_eq!(sim.pending(), 0);
        sim.cancel(3);
        assert_eq!(sim.tick_interval(3), None);
    }
}
