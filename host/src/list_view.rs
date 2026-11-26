//! The report projection keeps native indices private and uses Roc keys for identity.

use crate::Result;
use crate::wire::{Key, Kind, Node, Tree};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Column {
    pub(crate) key: String,
    pub(crate) heading: String,
    /// A fixed width in DIPs. None measures the heading and mounted cells.
    pub(crate) fixed_width: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) key: String,
    pub(crate) cells: Vec<String>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) columns: Vec<Column>,
    pub(crate) rows: Vec<Row>,
}

impl Report {
    pub(crate) fn read(tree: &Tree, root: Key) -> Result<Self> {
        let mut columns = Vec::new();
        let mut by_key = BTreeMap::new();
        let mut keys = BTreeSet::new();
        let mut rows = Vec::new();
        for (key, tag) in elements(tree, root) {
            let node = tree.node(key);
            match tag {
                "ui-list-column" if rows.is_empty() => {
                    let key_value = item_key(node)?;
                    if by_key.insert(key_value, columns.len()).is_some() {
                        return Err(format!("duplicate report column {key_value:?}"));
                    }
                    let fixed_width = match node.attr("list-width").unwrap_or("auto") {
                        "auto" => None,
                        value => {
                            Some(value.parse::<u32>().ok().filter(|&value| value > 0).ok_or(
                                "report column width must be Auto or a positive DIP value",
                            )?)
                        }
                    };
                    columns.push(Column {
                        key: key_value.into(),
                        heading: caption(tree, key)?,
                        fixed_width,
                    });
                }
                "ui-list-row" if !columns.is_empty() => {
                    let key_value = item_key(node)?;
                    if !keys.insert(key_value) {
                        return Err(format!("duplicate report row {key_value:?}"));
                    }
                    let mut cells = vec![None; columns.len()];
                    for (cell, tag) in elements(tree, key) {
                        if tag != "ui-list-cell" {
                            return Err(format!("report row {key_value:?} contains {tag}"));
                        }
                        let column = item_key(tree.node(cell))?;
                        let &index = by_key.get(column).ok_or_else(|| {
                            format!("report row {key_value:?} names unknown column {column:?}")
                        })?;
                        if cells[index].replace(caption(tree, cell)?).is_some() {
                            return Err(format!(
                                "report row {key_value:?} repeats column {column:?}"
                            ));
                        }
                    }
                    let cells = cells
                        .into_iter()
                        .zip(&columns)
                        .map(|(value, column)| {
                            value.ok_or_else(|| {
                                format!(
                                    "report row {key_value:?} is missing column {:?}",
                                    column.key
                                )
                            })
                        })
                        .collect::<Result<_>>()?;
                    rows.push(Row {
                        key: key_value.into(),
                        cells,
                    });
                }
                _ => return Err(format!("unexpected {tag} in the report")),
            }
        }
        if columns.is_empty() {
            return Err("a report requires at least one column".into());
        }
        if rows.len() > i32::MAX as usize || columns.len() > i32::MAX as usize {
            return Err("the report exceeds Win32 index limits".into());
        }
        Ok(Self { columns, rows })
    }
}

fn elements(tree: &Tree, parent: Key) -> impl Iterator<Item = (Key, &str)> {
    tree.node(parent)
        .children
        .iter()
        .filter_map(|&key| match &tree.node(key).kind {
            Kind::Element(tag) => Some((key, tag.as_str())),
            _ => None,
        })
}

fn item_key(node: &Node) -> Result<&str> {
    node.attr("list-key")
        .filter(|key| !key.is_empty() && !key.contains('\0'))
        .ok_or_else(|| {
            "report row, column and cell keys must be nonempty and contain no NUL".into()
        })
}

fn caption(tree: &Tree, key: Key) -> Result<String> {
    if elements(tree, key).next().is_some() {
        return Err("report headings and cells hold text only".into());
    }
    let text = tree.caption(key);
    if text.contains('\0') {
        return Err("report text contains a NUL".into());
    }
    Ok(text)
}

#[derive(Debug, PartialEq, Eq)]
enum Change {
    Delete(usize),
    Insert(usize),
    Cell { row: usize, column: usize },
}

/// The indices in the plan address the live list at the time of each change.
fn changes(old: &[Row], new: &[Row]) -> Vec<Change> {
    if old == new {
        return Vec::new();
    }
    let retained = retained_rows(old, new);
    let mut plan = Vec::new();
    for (index, row) in old.iter().enumerate().rev() {
        if !retained.contains_key(row.key.as_str()) {
            plan.push(Change::Delete(index));
        }
    }
    for (index, row) in new.iter().enumerate() {
        if let Some(old) = retained.get(row.key.as_str()) {
            for (column, (before, after)) in old.cells.iter().zip(&row.cells).enumerate() {
                if before != after {
                    plan.push(Change::Cell { row: index, column });
                }
            }
        } else {
            plan.push(Change::Insert(index));
        }
    }
    plan
}

/// Rows whose old order already matches the new order can stay mounted.
/// Keep a longest increasing subsequence of the surviving new indices.
fn retained_rows<'a>(old: &'a [Row], new: &[Row]) -> BTreeMap<&'a str, &'a Row> {
    let positions: BTreeMap<_, _> = new
        .iter()
        .enumerate()
        .map(|(index, row)| (row.key.as_str(), index))
        .collect();
    let mut tails: Vec<(usize, usize)> = Vec::new();
    let mut predecessors = vec![None; old.len()];
    for (old_index, row) in old.iter().enumerate() {
        let Some(&new_index) = positions.get(row.key.as_str()) else {
            continue;
        };
        let length = tails.partition_point(|&(_, tail)| tail < new_index);
        predecessors[old_index] = length.checked_sub(1).map(|prior| tails[prior].0);
        if let Some(tail) = tails.get_mut(length) {
            *tail = (old_index, new_index);
        } else {
            tails.push((old_index, new_index));
        }
    }
    let mut retained = BTreeMap::new();
    let mut current = tails.last().map(|&(old_index, _)| old_index);
    while let Some(index) = current {
        let row = &old[index];
        retained.insert(row.key.as_str(), row);
        current = predecessors[index];
    }
    retained
}

#[cfg(windows)]
mod native {
    use super::{Change, Report, Result, Row, changes};
    use crate::view::{scale, text_width, wide};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetDC, HFONT, ReleaseDC, SelectObject};
    use windows_sys::Win32::UI::Controls::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, SendMessageW};

    pub(crate) const ROW_DIP: i32 = 24;
    pub(crate) const HEADER_DIP: i32 = 26;
    pub(crate) const AUTO_CAP_DIP: i32 = 360;
    const MIN_COLUMN_DIP: i32 = 48;

    #[derive(Default)]
    pub(crate) struct ListView {
        items: Report,
        controlled: Option<String>,
        widths: Vec<i32>,
    }

    impl ListView {
        pub(crate) fn initialize(hwnd: HWND) {
            let styles = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
            send(
                hwnd,
                LVM_SETEXTENDEDLISTVIEWSTYLE,
                styles as usize,
                styles as isize,
            );
        }

        pub(crate) fn sync(
            &mut self,
            hwnd: HWND,
            font: HFONT,
            dpi: u32,
            desired: Report,
            selection: Option<&str>,
        ) -> Result<()> {
            let replaced_columns = self.items.columns != desired.columns;
            let top = self
                .row_at(send(hwnd, LVM_GETTOPINDEX, 0, 0) as i32)
                .map(str::to_owned);
            let focused = self.row_at(next(hwnd, LVNI_FOCUSED)).map(str::to_owned);
            let selected = selection.filter(|key| desired.rows.iter().any(|r| r.key == *key));
            let scroll_to_selection = selected.is_some() && self.controlled.as_deref() != selected;
            let resize_columns = self.items != desired;

            if replaced_columns {
                send(hwnd, LVM_DELETEALLITEMS, 0, 0);
                for index in (0..self.items.columns.len()).rev() {
                    native_ok(
                        send(hwnd, LVM_DELETECOLUMN, index, 0),
                        "delete a report column",
                    )?;
                }
                for (index, column) in desired.columns.iter().enumerate() {
                    let mut heading = wide(&column.heading);
                    let mut native = LVCOLUMNW {
                        mask: LVCF_TEXT,
                        pszText: heading.as_mut_ptr(),
                        ..LVCOLUMNW::default()
                    };
                    if send(hwnd, LVM_INSERTCOLUMNW, index, addr_of(&mut native)) < 0 {
                        return Err("could not insert a report column".into());
                    }
                }
                self.items.rows.clear();
                self.widths.clear();
            }

            let plan = changes(&self.items.rows, &desired.rows);
            for change in &plan {
                match *change {
                    Change::Delete(index) => {
                        native_ok(send(hwnd, LVM_DELETEITEM, index, 0), "delete a report row")?;
                    }
                    Change::Insert(index) => insert(hwnd, index, &desired.rows[index])?,
                    Change::Cell { row, column } => {
                        set_cell(hwnd, row, column, &desired.rows[row].cells[column])?;
                    }
                }
            }

            let anchor = top
                .as_deref()
                .and_then(|key| desired.rows.iter().position(|row| row.key == key));
            let focused = focused
                .as_deref()
                .and_then(|key| desired.rows.iter().position(|row| row.key == key));
            self.controlled = selected.map(str::to_owned);
            self.items = desired;
            self.restore(hwnd)?;
            if let Some(index) = focused
                && !scroll_to_selection
                && next(hwnd, LVNI_FOCUSED) != index as i32
            {
                state(hwnd, index, LVIS_FOCUSED, LVIS_FOCUSED)?;
            }
            if resize_columns {
                self.measure(hwnd, font, dpi)?;
            }
            if scroll_to_selection {
                if let Some(index) = self.controlled.as_deref().and_then(|key| self.index(key)) {
                    send(hwnd, LVM_ENSUREVISIBLE, index, 0);
                }
            } else if let Some(anchor) = anchor
                && (replaced_columns || plan.iter().any(|op| !matches!(op, Change::Cell { .. })))
            {
                scroll_to(hwnd, anchor, dpi);
            }
            Ok(())
        }

        pub(crate) fn measure(&mut self, hwnd: HWND, font: HFONT, dpi: u32) -> Result<()> {
            // SAFETY: This window lives on the calling thread. The font outlives
            // the synchronous measurement and the original font is restored.
            let widths = unsafe {
                let dc = GetDC(hwnd);
                if dc.is_null() {
                    return Err("could not measure report column text".into());
                }
                let original = SelectObject(dc, font);
                let widths = self
                    .items
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        if let Some(dip) = column.fixed_width {
                            return scale(dpi, dip.min(32767) as i32);
                        }
                        self.items
                            .rows
                            .iter()
                            .map(|row| text_width(dc, &row.cells[index]))
                            .fold(text_width(dc, &column.heading), i32::max)
                            .saturating_add(scale(dpi, 24))
                            .clamp(scale(dpi, MIN_COLUMN_DIP), scale(dpi, AUTO_CAP_DIP))
                    })
                    .collect::<Vec<_>>();
                SelectObject(dc, original);
                ReleaseDC(hwnd, dc);
                widths
            };
            for (index, &width) in widths.iter().enumerate() {
                if self.widths.get(index) != Some(&width) {
                    native_ok(
                        send(hwnd, LVM_SETCOLUMNWIDTH, index, width as isize),
                        "set a report column width",
                    )?;
                }
            }
            self.widths = widths;
            Ok(())
        }

        pub(crate) fn height(&self, hwnd: HWND, rows: i32, dpi: u32) -> i32 {
            let height = item_height(hwnd).unwrap_or_else(|| scale(dpi, ROW_DIP));
            let header = send(hwnd, LVM_GETHEADER, 0, 0) as HWND;
            let mut rect = RECT::default();
            // SAFETY: Win32 owns the optional header. The RECT is writable.
            let header_height =
                if !header.is_null() && unsafe { GetWindowRect(header, &mut rect) } != 0 {
                    (rect.bottom - rect.top).max(scale(dpi, HEADER_DIP))
                } else {
                    scale(dpi, HEADER_DIP)
                };
            header_height + rows.clamp(1, 64) * height + scale(dpi, 22)
        }

        pub(crate) fn row_at(&self, index: i32) -> Option<&str> {
            self.items
                .rows
                .get(usize::try_from(index).ok()?)
                .map(|row| row.key.as_str())
        }

        pub(crate) fn column_at(&self, index: i32) -> Option<&str> {
            self.items
                .columns
                .get(usize::try_from(index).ok()?)
                .map(|column| column.key.as_str())
        }

        pub(crate) fn has_row(&self, key: &str) -> bool {
            self.index(key).is_some()
        }

        pub(crate) fn has_column(&self, key: &str) -> bool {
            self.items.columns.iter().any(|column| column.key == key)
        }

        pub(crate) fn selection(&self, hwnd: HWND) -> Option<&str> {
            self.row_at(next(hwnd, LVNI_SELECTED))
        }

        pub(crate) fn selection_changed(&self, hwnd: HWND) -> bool {
            self.selection(hwnd) != self.controlled.as_deref()
        }

        pub(crate) fn row_keys(&self, hwnd: HWND) -> Result<Vec<&str>> {
            let count = send(hwnd, LVM_GETITEMCOUNT, 0, 0) as usize;
            if count != self.items.rows.len() {
                return Err("native report count differs from mounted keys".into());
            }
            Ok(self.items.rows.iter().map(|row| row.key.as_str()).collect())
        }

        pub(crate) fn restore(&self, hwnd: HWND) -> Result<()> {
            self.select(hwnd, self.controlled.as_deref())
        }

        pub(crate) fn select(&self, hwnd: HWND, key: Option<&str>) -> Result<()> {
            if self.selection(hwnd) == key {
                return Ok(());
            }
            if let Some(key) = key {
                let at = self
                    .index(key)
                    .ok_or_else(|| format!("unknown report row {key:?}"))?;
                state(
                    hwnd,
                    at,
                    LVIS_SELECTED | LVIS_FOCUSED,
                    LVIS_SELECTED | LVIS_FOCUSED,
                )
            } else {
                state(hwnd, usize::MAX, LVIS_SELECTED, 0)
            }
        }

        fn index(&self, key: &str) -> Option<usize> {
            self.items.rows.iter().position(|row| row.key == key)
        }
    }

    fn insert(hwnd: HWND, index: usize, row: &Row) -> Result<()> {
        let mut title = wide(&row.cells[0]);
        let mut item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: index as i32,
            pszText: title.as_mut_ptr(),
            ..LVITEMW::default()
        };
        if send(hwnd, LVM_INSERTITEMW, 0, addr_of(&mut item)) != index as isize {
            return Err("could not insert a report row at the requested index".into());
        }
        for (column, text) in row.cells.iter().enumerate().skip(1) {
            set_cell(hwnd, index, column, text)?;
        }
        Ok(())
    }

    fn set_cell(hwnd: HWND, row: usize, column: usize, text: &str) -> Result<()> {
        let mut value = wide(text);
        let mut item = LVITEMW {
            iSubItem: column as i32,
            pszText: value.as_mut_ptr(),
            ..LVITEMW::default()
        };
        native_ok(
            send(hwnd, LVM_SETITEMTEXTW, row, addr_of(&mut item)),
            "update report cell text",
        )
    }

    fn state(hwnd: HWND, index: usize, mask: u32, value: u32) -> Result<()> {
        let mut item = LVITEMW {
            state: value,
            stateMask: mask,
            ..LVITEMW::default()
        };
        native_ok(
            send(hwnd, LVM_SETITEMSTATE, index, addr_of(&mut item)),
            "restore report item state",
        )
    }

    fn next(hwnd: HWND, kind: u32) -> i32 {
        send(hwnd, LVM_GETNEXTITEM, usize::MAX, kind as isize) as i32
    }

    fn scroll_to(hwnd: HWND, anchor: usize, dpi: u32) {
        let top = send(hwnd, LVM_GETTOPINDEX, 0, 0) as i32;
        let height = item_height(hwnd).unwrap_or_else(|| scale(dpi, ROW_DIP));
        let shift = (anchor as i32 - top).saturating_mul(height);
        if shift != 0 {
            send(hwnd, LVM_SCROLL, 0, shift as isize);
        }
    }

    fn item_height(hwnd: HWND) -> Option<i32> {
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..RECT::default()
        };
        (send(hwnd, LVM_GETITEMRECT, 0, addr_of(&mut rect)) != 0)
            .then_some(rect.bottom - rect.top)
            .filter(|&height| height > 0)
    }

    fn addr_of<T>(value: &mut T) -> isize {
        std::ptr::from_mut(value) as isize
    }

    fn send(hwnd: HWND, message: u32, w_param: usize, l_param: isize) -> isize {
        // SAFETY: Callers use the window on its owning thread. Any pointed-to
        // message data remains live until the synchronous message returns.
        unsafe { SendMessageW(hwnd, message, w_param, l_param) }
    }

    fn native_ok(value: isize, operation: &str) -> Result<()> {
        if value == 0 {
            Err(format!("could not {operation}"))
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
pub(crate) use native::ListView;

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, cells: &[&str]) -> Row {
        Row {
            key: key.into(),
            cells: cells.iter().map(|cell| (*cell).into()).collect(),
        }
    }

    #[test]
    fn report_changes_preserve_unrelated_cells_and_use_live_indices() {
        let old = [
            row("gone", &["x", "y"]),
            row("a", &["A", "1"]),
            row("b", &["B", "2"]),
            row("c", &["C", "3"]),
        ];
        let new = [
            row("b", &["B", "2"]),
            row("a", &["A", "9"]),
            row("c", &["C", "3"]),
            row("new", &["N", "4"]),
        ];
        assert_eq!(
            changes(&old, &new),
            [
                Change::Delete(1),
                Change::Delete(0),
                Change::Insert(1),
                Change::Insert(3),
            ]
        );
        assert!(changes(&new, &new).is_empty());
        let updated = [
            row("a", &["A", "9"]),
            row("b", &["B", "2"]),
            row("c", &["C", "3"]),
            row("gone", &["x", "y"]),
        ];
        assert_eq!(
            changes(&old, &updated),
            [
                Change::Delete(0),
                Change::Cell { row: 0, column: 1 },
                Change::Insert(3),
            ]
        );
    }
}
