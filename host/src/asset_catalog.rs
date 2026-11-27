//! Asset declarations embedded by the app builder.

use crate::Result;
use crate::app_resource::embedded_app_data;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const MAX_CATALOG_BYTES: usize = 32 * 1024;
const ASSET_CATALOG_ID: u16 = 502;
static CATALOG: OnceLock<BTreeMap<&str, &str>> = OnceLock::new();

pub(crate) fn resolve(name: &str) -> Result<&str> {
    let Some(alias) = name.strip_prefix("asset-ref:") else {
        return Ok(name);
    };
    CATALOG
        .get_or_init(|| {
            embedded_app_data(ASSET_CATALOG_ID, MAX_CATALOG_BYTES)
                .ok()
                .flatten()
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
