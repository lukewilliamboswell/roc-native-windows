//! Restore native Windows Installer table order after changing Embala's MSI.

use msi::{ColumnType, Package};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

type Schemas = BTreeMap<String, Vec<(ColumnType, bool)>>;

pub(crate) fn schema(package: &Package<File>) -> Schemas {
    package
        .tables()
        .map(|table| {
            let columns = table
                .columns()
                .iter()
                .map(|column| (column.coltype(), column.is_primary_key()))
                .collect();
            (table.name().to_owned(), columns)
        })
        .collect()
}

pub(crate) fn restore(path: &Path, tables: &Schemas) -> io::Result<()> {
    let mut compound = cfb::open_rw(path)?;
    let streams = compound
        .read_root_storage()
        .filter_map(|entry| Some((table_name(entry.name())?, entry.path().to_owned())))
        .collect::<BTreeMap<_, _>>();
    let mut pool_header = [0; 4];
    let pool = streams
        .get("_StringPool")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing MSI string pool"))?;
    compound.open_stream(pool)?.read_exact(&mut pool_header)?;
    let string_width = if pool_header[3] & 0x80 != 0 { 3 } else { 2 };
    for (table, columns) in tables {
        let Some(path) = streams.get(table) else {
            continue; // An empty table may have no stream.
        };
        let mut stream = compound.open_stream(path)?;
        let mut data = Vec::new();
        stream.read_to_end(&mut data)?;
        let sorted = order_rows(&data, columns, string_width)?;
        if sorted != data {
            stream.seek(SeekFrom::Start(0))?;
            stream.write_all(&sorted)?;
        }
    }
    compound.flush()
}

fn table_name(stream: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz._";
    let mut name = String::new();
    for encoded in stream.strip_prefix('\u{4840}')?.chars() {
        match encoded as usize {
            code @ 0x3800..=0x47ff => {
                let pair = code - 0x3800;
                name.push(char::from(ALPHABET[pair & 63]));
                name.push(char::from(ALPHABET[pair >> 6]));
            }
            code @ 0x4800..=0x483f => name.push(char::from(ALPHABET[code - 0x4800])),
            _ => name.push(encoded),
        }
    }
    Some(name)
}

fn order_rows(
    data: &[u8],
    columns: &[(ColumnType, bool)],
    string_width: usize,
) -> io::Result<Vec<u8>> {
    let widths: Vec<_> = columns
        .iter()
        .map(|(kind, _)| match kind {
            ColumnType::Int16 | ColumnType::Binary => 2,
            ColumnType::Int32 => 4,
            ColumnType::Str(_) => string_width,
        })
        .collect();
    let row_width: usize = widths.iter().sum();
    if !data.len().is_multiple_of(row_width) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete MSI row",
        ));
    }
    let rows = data.len() / row_width;
    let mut end = 0;
    let columns: Vec<_> = widths
        .into_iter()
        .zip(columns.iter().map(|(_, key)| *key))
        .map(|(width, key)| {
            let start = end;
            end += width * rows;
            (&data[start..end], width, key)
        })
        .collect();
    // Streams are column-major. Windows Installer compares each primary key
    // as the stored little-endian integer, including string pool references.
    let mut order: Vec<_> = (0..rows).collect();
    order.sort_by(|&a, &b| {
        columns
            .iter()
            .filter(|(_, _, key)| *key)
            .map(|(data, width, _)| {
                let cell = |row| &data[row * width..(row + 1) * width];
                cell(a).iter().rev().cmp(cell(b).iter().rev())
            })
            .find(|ordering| !ordering.is_eq())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut sorted = Vec::with_capacity(data.len());
    for (data, width, _) in columns {
        for row in &order {
            sorted.extend_from_slice(&data[row * width..(row + 1) * width]);
        }
    }
    Ok(sorted)
}
