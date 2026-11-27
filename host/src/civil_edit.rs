use crate::Result;
use crate::accessibility::NativeHelp;
use crate::civil::{self, CivilFormat, Kind, Value};
use crate::style::native::{Paint, StyledWindow};
use crate::view::{child, composing, set_text, wide, window_text};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::HFONT;
use windows_sys::Win32::System::SystemServices::{SS_LEFT, SS_NOPREFIX};
use windows_sys::Win32::UI::Controls::EM_SETCUEBANNER;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, DestroyWindow, SendMessageW, WM_SETFONT, WS_TABSTOP,
};
use windows_sys::core::w;

pub(crate) enum Commit {
    Clean,
    Edited,
    Changed(Value),
}

pub(crate) struct CivilEdit {
    kind: Kind,
    format: CivilFormat,
    committed: Value,
    invalid: bool,
    help: String,
    native_help: NativeHelp,
    pub(crate) note: HWND,
    pub(crate) chooser: Option<HWND>,
    note_style: Option<StyledWindow>,
}

impl CivilEdit {
    pub(crate) fn new(parent: HWND, edit: HWND, font: HFONT, kind: Kind) -> Result<Self> {
        let mut entry = Self {
            kind,
            format: civil::native::format(kind),
            committed: Value::Unset,
            invalid: false,
            help: String::new(),
            native_help: NativeHelp::new(edit)?,
            note: child(parent, font, 0, w!("STATIC"), SS_LEFT | SS_NOPREFIX)?,
            note_style: None,
            chooser: None,
        };
        if kind == Kind::Date {
            let button = child(
                parent,
                font,
                0,
                w!("BUTTON"),
                WS_TABSTOP | BS_PUSHBUTTON as u32,
            )?;
            set_text(button, "Choose date");
            entry.chooser = Some(button);
        }
        entry.refresh_note(edit);
        Ok(entry)
    }

    pub(crate) fn font(&self, font: HFONT) {
        // SAFETY: The passive caption belongs to this UI thread.
        unsafe {
            SendMessageW(self.note, WM_SETFONT, font as usize, 0);
            if let Some(button) = self.chooser {
                SendMessageW(button, WM_SETFONT, font as usize, 0);
            }
        }
    }

    pub(crate) fn style(&mut self, paint: Paint) -> Result<()> {
        if let Some(style) = &mut self.note_style {
            style.update(paint);
        } else {
            self.note_style = Some(StyledWindow::new(self.note, paint)?);
        }
        Ok(())
    }

    pub(crate) fn refresh_locale(&mut self, hwnd: HWND) {
        let previous = self.format.format(self.committed);
        let clean = !self.invalid && window_text(hwnd) == previous && !composing(hwnd);
        self.format = civil::native::format(self.kind);
        if clean {
            self.restore(hwnd);
        } else {
            self.refresh_note(hwnd);
        }
    }

    pub(crate) fn value(&self) -> Value {
        self.committed
    }

    pub(crate) fn set_value(&mut self, hwnd: HWND, value: Value) {
        if value != self.committed {
            self.committed = value;
            self.restore(hwnd);
        }
    }

    pub(crate) fn needs_commit(&self, hwnd: HWND) -> bool {
        self.invalid || window_text(hwnd) != self.format.format(self.committed)
    }

    pub(crate) fn commit(&mut self, hwnd: HWND) -> Commit {
        if !self.needs_commit(hwnd) {
            return Commit::Clean;
        }
        match self.format.parse(&window_text(hwnd)) {
            Ok(value) => {
                if value == self.committed {
                    // Enter still belongs to the editor after reconciling an edited draft.
                    self.restore(hwnd);
                    Commit::Edited
                } else {
                    Commit::Changed(value)
                }
            }
            Err(_) => {
                self.invalid = true;
                self.refresh_note(hwnd);
                Commit::Edited
            }
        }
    }

    pub(crate) fn cancel(&mut self, hwnd: HWND) -> bool {
        if self.needs_commit(hwnd) {
            self.restore(hwnd);
            true
        } else {
            false
        }
    }

    fn restore(&mut self, hwnd: HWND) {
        let text = self.format.format(self.committed);
        if window_text(hwnd) != text {
            set_text(hwnd, &text);
        }
        self.invalid = false;
        self.refresh_note(hwnd);
    }

    pub(crate) fn help(&self) -> &str {
        &self.help
    }

    pub(crate) fn announce_help(&mut self) {
        self.native_help.announce();
    }

    pub(crate) fn invalid(&self) -> bool {
        self.invalid
    }

    fn refresh_note(&mut self, edit: HWND) {
        let example = self.format.example();
        let help = if self.invalid {
            let value = match self.kind {
                Kind::Date => "date",
                Kind::Time => "time",
            };
            format!("Enter a valid {value}. Example {example}")
        } else {
            format!("Example {example}")
        };
        if self.help != help {
            set_text(self.note, &help);
            self.native_help.update(&help);
            self.help = help;
        }
        let cue = wide(&example);
        // SAFETY: This message copies the live terminated UTF-16 caption synchronously.
        unsafe { SendMessageW(edit, EM_SETCUEBANNER, 0, cue.as_ptr() as isize) };
    }
}

impl Drop for CivilEdit {
    fn drop(&mut self) {
        // SAFETY: This child is owned by the editor and lives on its UI thread.
        unsafe {
            if let Some(button) = self.chooser {
                DestroyWindow(button);
            }
            DestroyWindow(self.note);
        }
    }
}
