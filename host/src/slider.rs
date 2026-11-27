use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Range {
    min: i32,
    max: i32,
    step: i32,
}

impl Range {
    pub(crate) fn new(min: i32, max: i32, step: i32) -> Result<Self> {
        if min >= max {
            return Err("slider minimum must be below its maximum".into());
        }
        if step <= 0 {
            return Err("slider step must be positive".into());
        }
        Ok(Self { min, max, step })
    }

    fn clamp(self, value: i32) -> i32 {
        value.clamp(self.min, self.max)
    }
}

#[cfg(windows)]
pub(crate) mod native {
    use super::Range;
    use crate::Result;
    use crate::wire::Node;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Controls::{
        TBM_SETLINESIZE, TBM_SETPOS, TBM_SETRANGEMAX, TBM_SETRANGEMIN,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_USER};

    const TBM_GETPOS: u32 = WM_USER;

    #[derive(Default)]
    pub(crate) struct Slider {
        range: Option<Range>,
        desired: i32,
    }

    impl Slider {
        pub(crate) fn sync(&mut self, hwnd: HWND, node: &Node) -> Result<()> {
            let read = |name, default| {
                node.attr(name)
                    .unwrap_or(default)
                    .parse::<i32>()
                    .map_err(|_| format!("invalid slider {name}"))
            };
            let range = Range::new(
                read("range-min", "0")?,
                read("range-max", "100")?,
                read("step", "1")?,
            )?;
            let desired = range.clamp(read("value", "0")?);
            if self.range != Some(range) {
                // SAFETY: These synchronous messages take signed position values, not pointers.
                unsafe {
                    if range.min > self.range.map_or(100, |prior| prior.max) {
                        SendMessageW(hwnd, TBM_SETRANGEMAX, 0, range.max as isize);
                        SendMessageW(hwnd, TBM_SETRANGEMIN, 1, range.min as isize);
                    } else {
                        SendMessageW(hwnd, TBM_SETRANGEMIN, 0, range.min as isize);
                        SendMessageW(hwnd, TBM_SETRANGEMAX, 1, range.max as isize);
                    }
                    SendMessageW(hwnd, TBM_SETLINESIZE, 0, range.step as isize);
                }
                self.range = Some(range);
            }
            self.desired = desired;
            self.restore(hwnd);
            Ok(())
        }

        pub(crate) fn restore(&self, hwnd: HWND) {
            if Self::actual(hwnd) != self.desired {
                Self::propose(hwnd, self.desired);
            }
        }

        pub(crate) fn change(&self, hwnd: HWND) -> Option<i32> {
            let value = Self::actual(hwnd);
            (value != self.desired).then_some(value)
        }

        pub(crate) fn actual(hwnd: HWND) -> i32 {
            // SAFETY: hwnd belongs to the mounted trackbar.
            unsafe { SendMessageW(hwnd, TBM_GETPOS, 0, 0) as i32 }
        }

        pub(crate) fn propose(hwnd: HWND, value: i32) {
            // SAFETY: The value is a signed position; the control clamps it to its native range.
            unsafe { SendMessageW(hwnd, TBM_SETPOS, 1, value as isize) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_bounds_and_clamps_a_signed_value() {
        assert!(Range::new(1, 0, 1).is_err());
        assert!(Range::new(0, 10, 0).is_err());
        let range = Range::new(-100_000, 300_000, 31).unwrap();
        assert_eq!(range.clamp(i32::MIN), -100_000);
        assert_eq!(range.clamp(120_000), 120_000);
        assert_eq!(range.clamp(i32::MAX), 300_000);
    }
}
