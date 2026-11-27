//! Asset declarations embedded by the app builder.

use crate::Result;
use std::collections::BTreeMap;
use std::ptr::null;
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{
    FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
};

const MAX_CATALOG_BYTES: usize = 32 * 1024;
const ASSET_CATALOG_ID: u16 = 502;
static CATALOG: OnceLock<BTreeMap<&str, &str>> = OnceLock::new();

pub(crate) fn resolve(name: &str) -> Result<&str> {
    let Some(alias) = name.strip_prefix("asset-ref:") else {
        return Ok(name);
    };
    CATALOG
        .get_or_init(|| {
            resource(ASSET_CATALOG_ID, MAX_CATALOG_BYTES)
                .and_then(parse)
                .unwrap_or_default()
        })
        .get(alias)
        .copied()
        .ok_or_else(|| format!("no packaged asset declared as {alias:?}"))
}

fn parse(bytes: &[u8]) -> Option<BTreeMap<&str, &str>> {
    let mut lines = std::str::from_utf8(bytes).ok()?.lines();
    if lines.next()? != "roc-windows-assets 1" {
        return None;
    }
    let mut entries = BTreeMap::new();
    for line in lines {
        let (name, path) = line.split_once('\t')?;
        if entries.len() == 64 || entries.insert(name, path).is_some() {
            return None;
        }
    }
    Some(entries)
}

fn resource(id: u16, max_bytes: usize) -> Option<&'static [u8]> {
    // SAFETY: The current executable stays mapped for the process lifetime.
    // Win32 encodes integer resource identifiers in the pointer's low word.
    unsafe {
        let module = GetModuleHandleW(null());
        if module.is_null() {
            return None;
        }
        let resource = FindResourceW(
            module,
            std::ptr::without_provenance(usize::from(id)),
            std::ptr::without_provenance(10),
        );
        if resource.is_null() {
            return None;
        }
        let size = SizeofResource(module, resource) as usize;
        if !(1..=max_bytes).contains(&size) {
            return None;
        }
        let loaded = LoadResource(module, resource);
        if loaded.is_null() {
            return None;
        }
        let data = LockResource(loaded);
        if data.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(data.cast::<u8>(), size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_requires_a_known_version_and_unique_names() {
        let catalog = parse(b"roc-windows-assets 1\nlogo\ticons/logo.png\nfont\tfont.ttf").unwrap();
        assert_eq!(catalog.get("logo"), Some(&"icons/logo.png"));
        assert!(parse(b"roc-windows-assets 2\nlogo\tlogo.png").is_none());
        assert!(parse(b"roc-windows-assets 1\nlogo\tx.png\nlogo\ty.png").is_none());
        assert_eq!(resolve("legacy.png").unwrap(), "legacy.png");
        assert!(resolve("asset-ref:unknown").is_err());
    }
}
