# Third-party notices

- The Roc platform and examples use the UPL-licensed `roc-signals` engine. The generated Rust ABI bindings come from the vendored `RustGlue.roc` generator. See the [roc-signals license](https://github.com/lukewilliamboswell/roc-signals/blob/main/LICENSE) and [Roc license](https://github.com/roc-lang/roc/blob/890d6c82229fc6aadbdcb886ee3e09946e9cbbca/LICENSE).
- The host uses the Rust standard library and the `windows-sys` and `windows-link` crates selected by `host/Cargo.lock`. See the [Rust license information](https://github.com/rust-lang/rust/blob/master/COPYRIGHT) and [windows-rs licenses](https://github.com/microsoft/windows-rs).
- Windows linker inputs come from Zig 0.17.1. The startup and runtime archives contain compiled open-source MinGW-w64, Zig, and LLVM libunwind code. The import archives are generated from MinGW-w64 DLL definition files. Windows supplies the DLL implementations. See the notices in the [Zig distribution](https://ziglang.org/download/), including `LICENSE`, `lib/libc/mingw/COPYING`, and `lib/libunwind/LICENSE.TXT`.
- `windows.roc` uses the Roc packages `basic-cli`, `weaver`, and `roc-zip` at the content-addressed URLs in its header.
- The separate MSI packaging utility uses `embala-msi` (MIT) and the crates in `packaging/Cargo.lock`. Its code runs on the packaging machine. MSI output contains the app payload and declarative Windows Installer data. See the [embala sources and license](https://github.com/hurricanehrndz/embala).

Before publishing binary releases, attach the corresponding license texts, component inventory, and provenance for the exact bundle. The repository ships source while native Windows and release verification are pending.
