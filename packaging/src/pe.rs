//! Read the execution marker without depending on Windows APIs on the packaging machine.

use std::io::{self, Read, Seek, SeekFrom};

const RT_RCDATA: u32 = 10;
// Keep this identifier in sync with CLI staging and host startup.
const EXECUTION_MARKER: u32 = 504;
const DIRECTORY_FLAG: u32 = 1 << 31;

pub(super) fn has_non_shipping_marker(mut image: impl Read + Seek) -> io::Result<bool> {
    let mut signature = [0; 2];
    match image.read_exact(&mut signature) {
        Ok(()) if signature == *b"MZ" => {}
        Ok(()) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
        Err(error) => return Err(error),
    }
    let offset = u64::from(u32_at(&read::<4>(&mut image, 0x3c)?, 0));
    let signature = read::<24>(&mut image, offset)?;
    if signature[..4] != *b"PE\0\0" {
        return Err(invalid_pe());
    }
    let sections = u16_at(&signature, 6);
    let optional_size = u64::from(u16_at(&signature, 20));
    let optional = offset + 24;
    let data_directory = match u16_at(&read::<2>(&mut image, optional)?, 0) {
        0x10b => 96,
        0x20b => 112,
        _ => return Err(invalid_pe()),
    };
    if optional_size < data_directory {
        return Err(invalid_pe());
    }
    let count = u32_at(&read::<4>(&mut image, optional + data_directory - 4)?, 0);
    if count < 3 {
        return Ok(false);
    }
    if optional_size < data_directory + 24 {
        return Err(invalid_pe());
    }
    let descriptor = read::<8>(&mut image, optional + data_directory + 16)?;
    let rva = u32_at(&descriptor, 0);
    let size = u64::from(u32_at(&descriptor, 4));
    if rva == 0 && size == 0 {
        return Ok(false);
    }
    if rva == 0 || size == 0 {
        return Err(invalid_pe());
    }
    for index in 0..sections {
        let section = read::<40>(&mut image, optional + optional_size + 40 * u64::from(index))?;
        let virtual_address = u32_at(&section, 12);
        let raw_size = u64::from(u32_at(&section, 16));
        let raw = u64::from(u32_at(&section, 20));
        if let Some(delta) = rva.checked_sub(virtual_address).map(u64::from)
            && delta + size <= raw_size
        {
            let resource = Resource {
                image: &mut image,
                offset: raw + delta,
                size,
            };
            return resource.has_marker();
        }
    }
    Err(invalid_pe())
}

struct Resource<'a, T> {
    image: &'a mut T,
    offset: u64,
    size: u64,
}

impl<T: Read + Seek> Resource<'_, T> {
    fn has_marker(mut self) -> io::Result<bool> {
        let Some(data) = self.find(0, RT_RCDATA)? else {
            return Ok(false);
        };
        if data & DIRECTORY_FLAG == 0 {
            return Err(invalid_pe());
        }
        // A present marker ID is never accepted in a shipping archive,
        // regardless of the marker payload or language.
        Ok(self
            .find(u64::from(data & !DIRECTORY_FLAG), EXECUTION_MARKER)?
            .is_some())
    }

    fn find(&mut self, table: u64, wanted: u32) -> io::Result<Option<u32>> {
        let header = self.read::<16>(table)?;
        let names = u64::from(u16_at(&header, 12));
        let ids = u64::from(u16_at(&header, 14));
        if table + 16 + 8 * (names + ids) > self.size {
            return Err(invalid_pe());
        }
        for index in names..names + ids {
            let entry = self.read::<8>(table + 16 + 8 * index)?;
            if u32_at(&entry, 0) == wanted {
                return Ok(Some(u32_at(&entry, 4)));
            }
        }
        Ok(None)
    }

    fn read<const SIZE: usize>(&mut self, relative: u64) -> io::Result<[u8; SIZE]> {
        if relative + SIZE as u64 > self.size {
            return Err(invalid_pe());
        }
        read(self.image, self.offset + relative)
    }
}

fn invalid_pe() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid executable resource directory",
    )
}

fn read<const SIZE: usize>(image: &mut (impl Read + Seek), offset: u64) -> io::Result<[u8; SIZE]> {
    let mut bytes = [0; SIZE];
    image.seek(SeekFrom::Start(offset))?;
    image.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture(marker: bool) -> Vec<u8> {
        let mut image = vec![0; 0x300];
        image[..2].copy_from_slice(b"MZ");
        write32(&mut image, 0x3c, 0x80);
        image[0x80..0x84].copy_from_slice(b"PE\0\0");
        write16(&mut image, 0x86, 1);
        write16(&mut image, 0x94, 240);
        let optional = 0x98;
        write16(&mut image, optional, 0x20b);
        write32(&mut image, optional + 108, 16);
        write32(&mut image, optional + 128, 0x2000);
        write32(&mut image, optional + 132, 0x100);
        let section = optional + 240;
        write32(&mut image, section + 12, 0x2000);
        write32(&mut image, section + 16, 0x100);
        write32(&mut image, section + 20, 0x200);

        write16(&mut image, 0x20e, 1);
        write32(&mut image, 0x210, RT_RCDATA);
        write32(&mut image, 0x214, DIRECTORY_FLAG | 24);
        write16(&mut image, 0x226, 1);
        write32(
            &mut image,
            0x228,
            if marker { EXECUTION_MARKER } else { 501 },
        );
        write32(&mut image, 0x22c, DIRECTORY_FLAG | 48);
        image
    }

    fn write32(image: &mut [u8], offset: usize, value: u32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write16(image: &mut [u8], offset: usize, value: u16) {
        image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn detects_the_execution_marker_without_confusing_the_app_title_or_unrelated_bytes() {
        assert!(has_non_shipping_marker(Cursor::new(fixture(true))).unwrap());
        let mut development = fixture(true);
        development[0x280..0x299].copy_from_slice(b"roc-windows-development-1");
        assert!(has_non_shipping_marker(Cursor::new(development)).unwrap());
        let mut shipping = fixture(false);
        shipping[0x280..0x294].copy_from_slice(b"roc-windows-scenario");
        assert!(!has_non_shipping_marker(Cursor::new(shipping)).unwrap());
        assert!(!has_non_shipping_marker(Cursor::new(b"demo executable")).unwrap());
    }

    #[test]
    fn rejects_a_pe_with_an_out_of_bounds_resource_table() {
        let mut image = fixture(true);
        write32(&mut image, 0x214, DIRECTORY_FLAG | 256);
        assert!(has_non_shipping_marker(Cursor::new(image)).is_err());
    }
}
