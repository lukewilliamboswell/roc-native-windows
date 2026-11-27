use crate::Result;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Progress {
    #[default]
    Idle,
    Indeterminate,
    Determinate {
        completed: u64,
        total: u64,
    },
}

const NATIVE_MAX: u32 = 10_000;

impl Progress {
    pub(crate) fn parse(wire: &str) -> Result<Self> {
        match wire {
            "" | "idle" => Ok(Self::Idle),
            "indeterminate" => Ok(Self::Indeterminate),
            counts => {
                let (completed, total) = counts
                    .split_once('/')
                    .ok_or("expected a progress mode or completed/total counts")?;
                let completed = completed
                    .parse::<u64>()
                    .map_err(|_| "invalid completed count")?;
                let total = total.parse::<u64>().map_err(|_| "invalid total count")?;
                if total == 0 {
                    Ok(Self::Idle)
                } else {
                    Ok(Self::Determinate {
                        completed: completed.min(total),
                        total,
                    })
                }
            }
        }
    }

    fn native(self) -> NativeProgress {
        match self {
            Self::Idle => NativeProgress::Hidden,
            Self::Indeterminate => NativeProgress::Marquee,
            Self::Determinate { completed, total } => {
                let scaled = (u128::from(completed) * u128::from(NATIVE_MAX)) / u128::from(total);
                NativeProgress::Position(scaled as u32)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NativeProgress {
    #[default]
    Hidden,
    Marquee,
    Position(u32),
}

#[cfg(windows)]
pub(crate) mod native {
    use super::{NATIVE_MAX, NativeProgress, Progress};
    use crate::Result;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Controls::{
        PBM_GETPOS, PBM_GETRANGE, PBM_SETMARQUEE, PBM_SETPOS, PBM_SETRANGE32, PBS_MARQUEE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, IsWindowVisible, SendMessageW, SetWindowLongPtrW,
    };

    #[derive(Default)]
    pub(crate) struct ProgressBar {
        mode: NativeProgress,
    }

    impl ProgressBar {
        pub(crate) fn initialize(hwnd: HWND) {
            // SAFETY: The determinate progress bar belongs to this window thread.
            unsafe { SendMessageW(hwnd, PBM_SETRANGE32, 0, NATIVE_MAX as isize) };
        }

        pub(crate) fn update(&mut self, hwnd: HWND, requested: Progress) {
            let next = requested.native();
            if next == self.mode {
                return;
            }
            // SAFETY: The style and messages target a live progress bar on its UI thread.
            unsafe {
                if self.mode == NativeProgress::Marquee {
                    SendMessageW(hwnd, PBM_SETMARQUEE, 0, 0);
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(PBS_MARQUEE as isize));
                } else if next == NativeProgress::Marquee {
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style | PBS_MARQUEE as isize);
                    SendMessageW(hwnd, PBM_SETMARQUEE, 1, 0);
                }
                if let NativeProgress::Position(pos) = next {
                    SendMessageW(hwnd, PBM_SETPOS, pos as usize, 0);
                }
            }
            self.mode = next;
        }

        pub(crate) fn visible(&self) -> bool {
            self.mode != NativeProgress::Hidden
        }

        pub(crate) fn expect(hwnd: HWND, expected: Progress) -> Result<()> {
            // SAFETY: This spec locator resolves to a live progress bar on its UI thread.
            let actual = unsafe {
                if IsWindowVisible(hwnd) == 0 {
                    NativeProgress::Hidden
                } else if GetWindowLongPtrW(hwnd, GWL_STYLE) & PBS_MARQUEE as isize != 0 {
                    NativeProgress::Marquee
                } else {
                    let low = SendMessageW(hwnd, PBM_GETRANGE, 1, 0);
                    let high = SendMessageW(hwnd, PBM_GETRANGE, 0, 0);
                    if low != 0 || high != NATIVE_MAX as isize {
                        return Err(format!("unexpected progress range {low} through {high}"));
                    }
                    NativeProgress::Position(SendMessageW(hwnd, PBM_GETPOS, 0, 0) as u32)
                }
            };
            let expected = expected.native();
            if actual != expected {
                return Err(format!(
                    "expected native progress {expected:?}, observed {actual:?}"
                ));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_zero_totals_and_caps_overshoot() {
        assert_eq!(Progress::parse("7/0"), Ok(Progress::Idle));
        assert_eq!(
            Progress::parse("27/10").unwrap().native(),
            NativeProgress::Position(NATIVE_MAX)
        );
        assert!(Progress::parse("minus/12").is_err());
    }

    #[test]
    fn scales_incomplete_and_complete_work_without_overflow() {
        assert_eq!(
            Progress::parse("1/3").unwrap().native(),
            NativeProgress::Position(3333)
        );
        assert_eq!(
            Progress::parse("0/42").unwrap().native(),
            NativeProgress::Position(0)
        );
        let total = u64::MAX;
        let incomplete = Progress::parse(&format!("{}/{total}", total - 1)).unwrap();
        let complete = Progress::parse(&format!("{total}/{total}")).unwrap();
        assert_eq!(
            incomplete.native(),
            NativeProgress::Position(NATIVE_MAX - 1)
        );
        assert_eq!(complete.native(), NativeProgress::Position(NATIVE_MAX));
        assert_eq!(
            Progress::parse("indeterminate").unwrap().native(),
            NativeProgress::Marquee
        );
    }
}
