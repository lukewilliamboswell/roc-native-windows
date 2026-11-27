use super::Date;
use crate::Result;
use crate::civil::{Value, native::locale_setting};
use crate::display_preferences::DisplayPreferences;
use crate::modal::{self, Modal};
use crate::style::native::{FontRole, Fonts};
use crate::view::{child, position, scale, set_text, wide};
use crate::windows::OwnedWindow;
use crate::wire::Key;
use std::ptr::{null, null_mut};
use windows::Win32::Foundation::HWND as ComWindow;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    CLSID_AccPropServices, IAccPropServices, Name_Property_GUID, PROPID_ACC_NAME,
};
use windows::core::PCWSTR;
use windows_sys::Win32::Foundation::{HWND, RECT, SYSTEMTIME};
use windows_sys::Win32::Globalization::{
    CAL_GREGORIAN, CAL_SMONTHNAME1, DATE_LONGDATE, GetCalendarInfoEx, GetDateFormatEx,
    LOCALE_ICALENDARTYPE, LOCALE_IFIRSTDAYOFWEEK, LOCALE_SABBREVDAYNAME1, LOCALE_SDAYNAME1,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::SystemServices::SS_CENTER;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetActiveWindow, GetFocus, IsWindowEnabled, SetFocus, VK_DOWN, VK_END, VK_ESCAPE,
    VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

#[derive(Clone, Copy)]
pub(crate) struct Origin {
    pub(crate) owner: HWND,
    pub(crate) field: Key,
    pub(crate) edit: HWND,
    pub(crate) button: HWND,
}

pub(crate) enum Action {
    Ignore,
    Handled,
    Close,
    Choose(Value),
}

pub(crate) struct Calendar {
    pub(crate) origin: Origin,
    date: Date,
    first_weekday: u16,
    settings: DisplayPreferences,
    month: HWND,
    previous: HWND,
    next: HWND,
    weekdays: [HWND; 7],
    days: [HWND; 31],
    dismiss: HWND,
    names: Option<IAccPropServices>,
    guard: Modal,
    window: OwnedWindow,
    fonts: Fonts,
}

impl Calendar {
    pub(crate) fn open(
        origin: Origin,
        title: &str,
        date: Value,
        settings: DisplayPreferences,
        proc: WNDPROC,
    ) -> Result<Self> {
        let window = OwnedWindow(modal::create(origin.owner, proc, title)?);
        let dialog = window.0;
        // SAFETY: This dialog lives on its owner's UI thread.
        let dpi = unsafe {
            let style = GetWindowLongPtrW(dialog, GWL_STYLE);
            SetWindowLongPtrW(
                dialog,
                GWL_STYLE,
                style & !((WS_THICKFRAME | WS_VSCROLL) as isize),
            );
            GetDpiForWindow(origin.owner).max(96)
        };
        let fonts = Fonts::new(dpi, settings.text_scale)?;
        let font = fonts.get(FontRole::Body);
        let caption = || child(dialog, font, 0, w!("STATIC"), SS_CENTER);
        let button = |text: &str, tab| -> Result<_> {
            let handle = child(dialog, font, 0, w!("BUTTON"), BS_PUSHBUTTON as u32 | tab)?;
            set_text(handle, text);
            Ok(handle)
        };
        let month = caption()?;
        let previous = button("Previous month", WS_TABSTOP)?;
        let next = button("Next month", WS_TABSTOP)?;
        let mut weekdays = [null_mut(); 7];
        for label in &mut weekdays {
            *label = caption()?;
        }
        let mut days = [null_mut(); 31];
        for (day, button_handle) in days.iter_mut().enumerate() {
            *button_handle = button(&(day + 1).to_string(), 0)?;
        }
        let dismiss = button("Cancel", WS_TABSTOP)?;
        let date = Date::from_value(date).unwrap_or_else(today);
        let first_weekday = first_weekday();
        // SAFETY: The host UI thread has a COM apartment. Standard buttons
        // remain operable if native property annotation is unavailable.
        let names =
            unsafe { CoCreateInstance(&CLSID_AccPropServices, None, CLSCTX_INPROC_SERVER).ok() };
        let guard = Modal::begin(origin.owner, dialog);
        let picker = Self {
            origin,
            date,
            first_weekday,
            settings,
            month,
            previous,
            next,
            weekdays,
            days,
            dismiss,
            names,
            guard,
            window,
            fonts,
        };
        picker.refresh_month();
        picker.position()?;
        picker.guard.show();
        if unsafe { GetActiveWindow() == picker.handle() } {
            picker.focus_day();
        }
        Ok(picker)
    }

    pub(crate) fn handle(&self) -> HWND {
        self.window.0
    }

    pub(crate) fn close(mut self, restore: bool) {
        let restore = restore && self.guard.can_restore_focus();
        self.guard.release_owner();
        if let Some(service) = self.names.take() {
            for button in self.days {
                // SAFETY: This guard owns the annotations for these child HWNDs.
                unsafe {
                    let _ = service.ClearHwndProps(
                        ComWindow(button),
                        OBJID_CLIENT as u32,
                        0,
                        &[Name_Property_GUID, PROPID_ACC_NAME],
                    );
                }
            }
        }
        drop(self.window);
        self.guard.finish(restore);
    }

    pub(crate) fn settings_changed(&mut self, settings: DisplayPreferences) -> Result<()> {
        // SAFETY: The dialog and its children are still mounted.
        let dpi = unsafe { GetDpiForWindow(self.handle()) }.max(96);
        let fonts = Fonts::new(dpi, settings.text_scale)?;
        let font = fonts.get(FontRole::Body);
        for hwnd in [self.month, self.previous, self.next, self.dismiss]
            .into_iter()
            .chain(self.weekdays)
            .chain(self.days)
        {
            unsafe { SendMessageW(hwnd, WM_SETFONT, font as usize, 1) };
        }
        self.fonts = fonts;
        self.settings = settings;
        self.first_weekday = first_weekday();
        self.refresh_month();
        self.position()
    }

    pub(crate) fn command(&mut self, source: HWND, id: u16) -> Result<Action> {
        if source == self.dismiss || (source.is_null() && id == IDCANCEL as u16) {
            return Ok(Action::Close);
        }
        if source == self.previous || source == self.next {
            let offset = if source == self.previous { -1 } else { 1 };
            if let Some(date) = self.date.change_month(offset) {
                self.set_date(date)?;
                // At the range boundaries, the just-pressed command may disappear
                // from the tab sequence. Keep navigation in the dialog.
                if unsafe { IsWindowEnabled(source) == 0 } {
                    self.focus_day();
                }
            }
            return Ok(Action::Handled);
        }
        if let Some(index) = self.days.iter().position(|&hwnd| hwnd == source) {
            let day = u16::try_from(index + 1).unwrap();
            if day <= self.date.month_days() {
                return Ok(Action::Choose(Date { day, ..self.date }.value()));
            }
        }
        Ok(Action::Ignore)
    }

    pub(crate) fn key(&mut self, key: u16, repeated: bool) -> Result<Action> {
        if key == VK_ESCAPE {
            return Ok(if repeated {
                Action::Handled
            } else {
                Action::Close
            });
        }
        let focus = unsafe { GetFocus() };
        if key == VK_RETURN {
            return if repeated {
                Ok(Action::Handled)
            } else {
                self.command(focus, 0)
            };
        }
        let Some(index) = self.days.iter().position(|&day| day == focus) else {
            return Ok(Action::Ignore);
        };
        let date = Date {
            day: (index + 1) as u16,
            ..self.date
        };
        let next = match key {
            VK_LEFT => Some(date.change_day(-1)),
            VK_RIGHT => Some(date.change_day(1)),
            VK_UP => Some(date.change_day(-7)),
            VK_DOWN => Some(date.change_day(7)),
            VK_HOME => Some(Date { day: 1, ..date }),
            VK_END => Some(Date {
                day: date.month_days(),
                ..date
            }),
            VK_PRIOR => date.change_month(-1),
            VK_NEXT => date.change_month(1),
            _ => return Ok(Action::Ignore),
        };
        if let Some(date) = next {
            self.set_date(date)?;
            self.focus_day();
        }
        Ok(Action::Handled)
    }

    fn set_date(&mut self, date: Date) -> Result<()> {
        let change_month = (self.date.year, self.date.month) != (date.year, date.month);
        self.date = date;
        if change_month {
            self.refresh_month();
            self.position()?;
        } else {
            self.tab_stop();
        }
        Ok(())
    }

    fn focus_day(&self) {
        // SAFETY: The selected native day button is live, visible and enabled.
        unsafe { SetFocus(self.days[usize::from(self.date.day - 1)]) };
    }

    fn tab_stop(&self) {
        for (index, &button) in self.days.iter().enumerate() {
            // SAFETY: Day button identity survives within this dialog.
            unsafe {
                let style = GetWindowLongPtrW(button, GWL_STYLE);
                let next = if index + 1 == usize::from(self.date.day) {
                    style | WS_TABSTOP as isize
                } else {
                    style & !(WS_TABSTOP as isize)
                };
                if style != next {
                    SetWindowLongPtrW(button, GWL_STYLE, next);
                }
            }
        }
    }

    fn refresh_month(&self) {
        let month = month_name(self.date);
        set_text(self.month, &format!("{month} {}", self.date.year));
        for (index, &label) in self.weekdays.iter().enumerate() {
            let day = (u32::from(self.first_weekday) + index as u32) % 7;
            set_text(label, &locale_setting(LOCALE_SABBREVDAYNAME1 + day));
        }
        let active_calendar = locale_setting(LOCALE_ICALENDARTYPE);
        for (index, &button) in self
            .days
            .iter()
            .enumerate()
            .take(usize::from(self.date.month_days()))
        {
            let date = Date {
                day: (index + 1) as u16,
                ..self.date
            };
            if let Some(service) = &self.names {
                let name = wide(&day_name(date, &month, &active_calendar));
                // SAFETY: Both the day HWND and terminated UTF-16 name are live.
                unsafe {
                    for property in [Name_Property_GUID, PROPID_ACC_NAME] {
                        let _ = service.SetHwndPropStr(
                            ComWindow(button),
                            OBJID_CLIENT as u32,
                            0,
                            property,
                            PCWSTR(name.as_ptr()),
                        );
                    }
                }
            }
        }
        self.tab_stop();
        // SAFETY: The two navigation buttons are controls of this dialog.
        unsafe {
            EnableWindow(
                self.previous,
                i32::from(self.date.change_month(-1).is_some()),
            );
            EnableWindow(self.next, i32::from(self.date.change_month(1).is_some()));
        }
    }

    fn position(&self) -> Result<()> {
        // SAFETY: Both HWNDs belong to the UI thread.
        let dpi = unsafe { GetDpiForWindow(self.origin.owner) }.max(96);
        let dip = |amount| scale(dpi, amount);
        let text_size = |amount| {
            scale(
                dpi,
                (f64::from(amount) * self.settings.text_scale).round() as i32,
            )
        };
        let edge = dip(12);
        let line = dip(28).max(text_size(23));
        let cell_h = dip(33).max(text_size(27));
        let cell_min = dip(50).max(text_size(40));
        let height = edge * 2 + line * 4 + cell_h * 6;
        modal::position(
            self.handle(),
            self.origin.owner,
            (edge * 2 + cell_min * 7, height),
        )?;
        let mut client = RECT::default();
        // SAFETY: Position just sized the live dialog.
        unsafe { GetClientRect(self.handle(), &mut client) };
        let body_width = (client.right - edge * 2).max(7);
        let row = |index| edge + line * index;
        let half = body_width / 2;
        position(self.month, HWND_TOP, edge, row(0), body_width, line);
        position(self.previous, self.month, edge, row(1), half - dip(3), line);
        position(
            self.next,
            self.previous,
            edge + half + dip(3),
            row(1),
            half - dip(3),
            line,
        );
        let cell_w = body_width / 7;
        let mut after = self.next;
        for (index, day) in self.weekdays.into_iter().enumerate() {
            position(
                day,
                after,
                edge + index as i32 * cell_w,
                row(2),
                cell_w,
                line,
            );
            after = day;
        }
        let offset = i32::from((self.date.first_weekday() + 7 - self.first_weekday) % 7);
        for (index, day) in self.days.into_iter().enumerate() {
            if index >= usize::from(self.date.month_days()) {
                unsafe { ShowWindow(day, SW_HIDE) };
                continue;
            }
            let at = offset + index as i32;
            position(
                day,
                after,
                edge + (at % 7) * cell_w,
                row(3) + (at / 7) * cell_h,
                cell_w - dip(2),
                cell_h - dip(2),
            );
            after = day;
        }
        position(
            self.dismiss,
            after,
            edge + body_width - half,
            row(3) + cell_h * 6,
            half,
            line,
        );
        Ok(())
    }
}

fn first_weekday() -> u16 {
    locale_setting(LOCALE_IFIRSTDAYOFWEEK)
        .parse::<u16>()
        .ok()
        .filter(|day| *day < 7)
        .unwrap_or(0)
}

fn today() -> Date {
    let mut time = SYSTEMTIME::default();
    // SAFETY: The clock API writes a full SYSTEMTIME.
    unsafe { GetLocalTime(&mut time) };
    Date::from_value(
        Value::date(time.wYear, time.wMonth, time.wDay).unwrap_or(Value::Date {
            year: super::FIRST_YEAR,
            month: 1,
            day: 1,
        }),
    )
    .unwrap()
}

fn month_name(date: Date) -> String {
    let mut text = [0u16; 128];
    // SAFETY: Gregorian names are available independently of the user's active calendar.
    let len = unsafe {
        GetCalendarInfoEx(
            null(),
            CAL_GREGORIAN,
            null(),
            CAL_SMONTHNAME1 + u32::from(date.month - 1),
            text.as_mut_ptr(),
            text.len() as i32,
            null_mut(),
        )
    };
    if len > 1 {
        String::from_utf16_lossy(&text[..len as usize - 1])
    } else {
        date.month.to_string()
    }
}

fn day_name(date: Date, month: &str, active_calendar: &str) -> String {
    if matches!(active_calendar, "1" | "2") {
        let time = SYSTEMTIME {
            wYear: date.year,
            wMonth: date.month,
            wDay: date.day,
            ..SYSTEMTIME::default()
        };
        let mut text = [0u16; 256];
        // SAFETY: The date is a validated Gregorian SYSTEMTIME and the buffer is writable.
        let len = unsafe {
            GetDateFormatEx(
                null(),
                DATE_LONGDATE,
                &time,
                null(),
                text.as_mut_ptr(),
                text.len() as i32,
                null(),
            )
        };
        if len > 1 {
            return String::from_utf16_lossy(&text[..len as usize - 1]);
        }
    }
    let weekday = (u32::from(date.first_weekday()) + u32::from(date.day - 1)) % 7;
    format!(
        "{}, {} {month} {}",
        locale_setting(LOCALE_SDAYNAME1 + weekday),
        date.day,
        date.year
    )
}
