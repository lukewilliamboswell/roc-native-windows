//! Framing shared by the released CLI and the app's Windows host.

use crate::Result;

pub(crate) const PROTOCOL: &str = "roc-windows-spec 1";
pub(crate) const MAX_PLAN_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Test,
    Demo,
}

#[derive(Debug)]
pub(crate) struct Step<'a> {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) command: &'a str,
    pub(crate) args: Vec<&'a str>,
}

impl Step<'_> {
    pub(crate) fn problem(&self, detail: impl std::fmt::Display) -> String {
        format!("{}:{}: {}: {detail}", self.line, self.column, self.command)
    }
}

pub(crate) struct Plan<'a> {
    pub(crate) mode: Mode,
    pub(crate) name: &'a str,
    pub(crate) steps: Vec<Step<'a>>,
}

impl<'a> Plan<'a> {
    pub(crate) fn read(text: &'a str) -> Result<Self> {
        if text.len() as u64 > MAX_PLAN_BYTES || !text.ends_with('\0') {
            return Err("invalid or oversized spec plan".into());
        }
        let mut fields = text.split_terminator('\0');
        if fields.next() != Some(PROTOCOL) {
            return Err(format!("expected protocol {PROTOCOL}"));
        }
        let mode = match fields.next() {
            Some("test") => Mode::Test,
            Some("demo") => Mode::Demo,
            _ => return Err("spec plan has an invalid mode".into()),
        };
        let name = fields
            .next()
            .filter(|name| !name.is_empty())
            .ok_or("spec plan lacks a name")?;
        let mut steps = Vec::new();
        let mut setup_ended = false;
        while let Some(line) = fields.next() {
            let line = positive(Some(line)).ok_or("invalid spec plan line")?;
            let column = positive(fields.next()).ok_or("invalid spec plan column")?;
            let count = positive(fields.next()).ok_or("invalid spec plan field count")?;
            let command = fields
                .next()
                .filter(|name| !name.is_empty())
                .ok_or("missing spec plan command")?;
            let args: Vec<_> = fields.by_ref().take(count - 1).collect();
            let step = Step {
                line,
                column,
                command,
                args,
            };
            if step.args.len() != count - 1 {
                return Err(step.problem("truncated arguments"));
            }
            if command == "expect-exit"
                && (!step.args.is_empty() || fields.clone().next().is_some())
            {
                return Err(step.problem(
                    "expect-exit takes no arguments and must be the final scenario step",
                ));
            }
            if command.starts_with("setup-") {
                if setup_ended {
                    return Err(step.problem("setup must precede scenario steps"));
                }
            } else {
                setup_ended = true;
            }
            steps.push(step);
        }
        if !setup_ended {
            return Err("spec plan lacks scenario steps".into());
        }
        Ok(Self { mode, name, steps })
    }
}

/// The first locator components can name native surfaces. The remaining
/// pairs describe ordinary Win32 controls in one surface's content tree.
#[derive(Clone, Default)]
pub(crate) struct Locator<'a> {
    pub(crate) window: Option<&'a str>,
    pub(crate) dialog: Option<&'a str>,
    pub(crate) controls: Vec<(&'a str, &'a str)>,
}

impl<'a> Locator<'a> {
    pub(crate) fn read<'fields>(fields: &'fields [&'a str]) -> Result<(Self, &'fields [&'a str])> {
        let (count, rest) = fields.split_first().ok_or("step needs a locator")?;
        let width = positive(Some(count))
            .and_then(|count| count.checked_mul(2))
            .ok_or("invalid locator size")?;
        let (encoded, args) = rest.split_at_checked(width).ok_or("truncated locator")?;
        let mut selectors = encoded.as_chunks::<2>().0;
        let mut locator = Self::default();
        if let [["window", title], rest @ ..] = selectors {
            locator.window = Some(*title);
            selectors = rest;
        }
        if let [["dialog", title], rest @ ..] = selectors {
            locator.dialog = Some(*title);
            selectors = rest;
        }
        if [locator.window, locator.dialog]
            .into_iter()
            .flatten()
            .any(str::is_empty)
        {
            return Err("window and dialog locators need a title".into());
        }
        for &[kind, label] in selectors {
            if matches!(kind, "window" | "dialog") {
                return Err("a window or dialog scope must precede control locators".into());
            }
            locator.controls.push((kind, label));
        }
        Ok((locator, args))
    }

    pub(crate) fn names_surface(&self) -> bool {
        self.window.is_some() || self.dialog.is_some()
    }
}

fn positive(field: Option<&str>) -> Option<usize> {
    field?.parse().ok().filter(|&n| n != 0)
}

pub(crate) fn valid_file_name(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c < ' ' || [':', '/', '\\', '<', '>', '"', '|', '?', '*'].contains(&c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    !["CON", "NUL", "PRN", "AUX", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        && !["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(fields: &[&str]) -> String {
        let mut text = fields.join("\0");
        text.push('\0');
        text
    }

    #[test]
    fn reads_source_locations_and_empty_arguments() {
        let source = plan(&[
            PROTOCOL,
            "test",
            "fill",
            "1",
            "2",
            "1",
            "setup-live-effects",
            "3",
            "4",
            "5",
            "fill",
            "1",
            "text-field",
            "Title",
            "",
        ]);
        let parsed = Plan::read(&source).unwrap();
        assert_eq!(parsed.mode, Mode::Test);
        assert_eq!(parsed.name, "fill");
        assert_eq!(parsed.steps[1].args, ["1", "text-field", "Title", ""]);
        assert_eq!(parsed.steps[1].problem("failure"), "3:4: fill: failure");
    }

    #[test]
    fn artifacts_require_a_windows_file_name() {
        for name in ["..", "not/a/name", "aux.bmp", "COM¹", "photo."] {
            assert!(!valid_file_name(name));
        }
        for name in ["Welcome screen", "approved.v2", "résumé"] {
            assert!(valid_file_name(name));
        }
    }

    #[test]
    fn separates_surface_scopes_from_controls_and_arguments() {
        let (locator, args) = Locator::read(&[
            "3",
            "window",
            "Main",
            "dialog",
            "Notes",
            "text-area",
            "Notes",
            "draft",
        ])
        .unwrap();
        assert_eq!(locator.window, Some("Main"));
        assert_eq!(locator.dialog, Some("Notes"));
        assert_eq!(locator.controls, [("text-area", "Notes")]);
        assert_eq!(args, ["draft"]);
        assert!(locator.names_surface());
        assert!(Locator::read(&["2", "button", "Open", "window", "Main"]).is_err());
        assert!(Locator::read(&["1", "window", ""]).is_err());
        assert!(Locator::read(&["2", "dialog", "Unfinished"]).is_err());
        let (legacy, _) = Locator::read(&["1", "button", "dialog"]).unwrap();
        assert!(!legacy.names_surface());
        assert_eq!(legacy.controls, [("button", "dialog")]);
    }

    #[test]
    fn accepts_exit_only_as_a_final_argument_free_step() {
        let terminal = plan(&[PROTOCOL, "test", "close", "1", "2", "1", "expect-exit"]);
        assert!(Plan::read(&terminal).is_ok());
        let early = plan(&[
            PROTOCOL,
            "test",
            "close",
            "1",
            "2",
            "1",
            "expect-exit",
            "2",
            "2",
            "1",
            "await-work",
        ]);
        assert!(Plan::read(&early).is_err());
        let extra = plan(&[
            PROTOCOL,
            "test",
            "close",
            "1",
            "2",
            "2",
            "expect-exit",
            "later",
        ]);
        assert!(Plan::read(&extra).is_err());
    }

    #[test]
    fn rejects_incompatible_and_incomplete_plans() {
        for fields in [
            &["v0", "test", "bad"][..],
            &[PROTOCOL, "test", "no-steps"],
            &[PROTOCOL, "test", "truncated", "1", "2", "2", "wait"],
            &[
                PROTOCOL,
                "test",
                "late-setup",
                "1",
                "2",
                "1",
                "await-work",
                "1",
                "2",
                "1",
                "setup-live-effects",
            ],
        ] {
            assert!(Plan::read(&plan(fields)).is_err());
        }
    }
}
