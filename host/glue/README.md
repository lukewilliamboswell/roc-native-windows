# Rust glue generator

`RustGlue.roc` is the unmodified generator from the Roc revision used by the Blueprint tooling compiler.

- Revision `890d6c82229fc6aadbdcb886ee3e09946e9cbbca`
- SHA-256 `ee2d047aded74d661009a37c20f52b3506d87522401597b3e8457cd4dec340f9`
- [Upstream source](https://github.com/roc-lang/roc/blob/890d6c82229fc6aadbdcb886ee3e09946e9cbbca/src/glue/src/RustGlue.roc)
- [UPL 1.0 license](LICENSE)

Update the generator from the selected compiler revision when changing the tooling compiler. Regenerate `host/src/roc_platform_abi.rs` only after reviewing the generator and ABI changes. See the [contributor guide](../../docs/contributing.adoc#build-the-platform).
