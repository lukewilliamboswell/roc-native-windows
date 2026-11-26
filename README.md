# roc-native-windows

Build native Windows desktop apps in Roc. `roc-signals` owns the application state and sends incremental UI updates to a Rust host. The host renders Win32 controls and sends Windows events back to Roc.

This is an early development checkout. Windows x64 and ARM64 are cross-build targets. Hosted CI is disabled while the initial implementation is developed locally. Native Windows testing on clean installs is still pending.

## Build from macOS

Use the `nightly-2026-11-26-890d6c8` Roc compiler, Blueprint 0.13.0, Zig 0.17.1, and Rustup. Rustup selects the toolchain and Windows targets from `rust-toolchain.toml`.

```sh
export PATH="$HOME/roc_nightly-macos_apple_silicon-2026-11-26-890d6c8:$PATH"
blueprint run check
blueprint run build
blueprint run bundle
```

The check runs portable Rust tests, Rust linting for macOS and both Windows targets, Roc tests, and Windows cross-builds. The build writes the example executables to `examples/<name>/build/<target>/`. `bundle` writes the platform bundle to `dist/platform/`.

The host links against Win32 DLLs already supplied by Windows. Zig provides the GNU startup and runtime inputs, generates import libraries from its bundled definitions, and compiles the project's Common Controls v6 and DPI manifest. The Rust host uses bindings from `windows-rs`. The developer build uses these tools to prepare the platform. An app author using a platform bundle needs only Roc to build the app.

## Apps and examples

Use `windows.roc` from an app directory. For example, from this checkout on macOS:

```sh
cd examples/counter
roc ../../windows.roc -- test
roc ../../windows.roc -- package --target x64mingw
```

This creates `dist/counter-x64mingw.zip` with the Windows executable and its `assets/` directory. Use `--target arm64mingw` for ARM64. On Windows, `run` defaults to the current architecture. Pass `--help` for all commands. Use `init --platform <platform/main.roc or bundle URL> <app-directory>` for a new app. Every example renders a bundled bitmap using `Ui.asset_image`.

| Example | Behavior |
| --- | --- |
| Counter | Button actions update local state and a text label |
| Todos | Native text input and checkboxes update a persisted list |
| Workers | Compute in a separate Roc worker while the window continues handling input |
| Files | Choose a file and compute its SHA-256 digest |
| HTTP | Fetch and display a bounded response preview with WinHTTP |

## Verification

macOS is the development host for the repository, build pipeline, and portable tests. Validate launch, input, focus, DPI, Narrator, file dialogs, networking, and packaging on Windows. Run the packaged examples on clean x64 and ARM64 installations to confirm the app bundle needs no separate runtime.
