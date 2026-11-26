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

Release assets include a content-addressed CLI bundle that Roc can run directly from a URL. Use the CLI bundle URL from the release to bootstrap a project.

```sh
roc run "$WINDOWS_TOOL_URL" -- init my-app
cd my-app
roc run windows.roc -- package
roc run windows.roc -- package --format zip
```

The scaffold points at that release's platform bundle. MSI packaging downloads the matching packager for the build machine and checks its SHA-256 before running it. Roc is the only toolchain app authors install. GitHub release URLs use HTTPS. The release task also accepts HTTP on loopback for testing.

Use `windows.roc` from an app directory. For example, from this checkout on macOS:

```sh
blueprint run packager
export ROC_WINDOWS_MSI="$PWD/packaging/target/release/roc-windows-msi"
cd examples/counter
roc run ../../windows.roc -- test
roc run ../../windows.roc -- package --target x64mingw
```

This creates `dist/counter-x64mingw.msi`. The MSI installs into Program Files and adds a Start menu shortcut. Set `--id` to your stable reverse-DNS app ID and provide `--app-version` and `--publisher` for release builds. The defaults are for local development. MSI metadata and asset filenames currently use ASCII. Both formats carry the executable, `assets/`, and any top-level `LICENSE` or `THIRD_PARTY_LICENSES.md` included by the app author.

Use `--target arm64mingw` for ARM64. On Windows, `run` defaults to the current architecture. Pass `--help` for all commands. Use `init --platform <platform/main.roc or bundle URL> <app-directory>` for a custom platform. Every example renders a bundled BMP using `Ui.asset_image`.

| Example | Behavior |
| --- | --- |
| Counter | Button actions update local state and a text label |
| Todos | Native text input and checkboxes update a persisted list |
| Workers | Compute in a separate Roc worker while the window continues handling input |
| Files | Choose a file and compute its SHA-256 digest |
| HTTP | Fetch and display a bounded response preview with WinHTTP |

Cancelling a running worker discards its result. A pure Roc computation can still finish in the background, and a Roc crash in a worker exits the process.

## Prepare a release

On Apple Silicon macOS, add the Intel macOS and Linux musl Rust targets alongside the Windows targets in `rust-toolchain.toml`. Build the platform bundle, then pass its generated filename and the proposed release asset base URL to the release task.

```sh
rustup target add x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl
blueprint run bundle
blueprint run release -- dist/platform/HASH.tar.zst "$RELEASE_BASE_URL" dist/release
```

The output includes the platform bundle, standalone `windows.roc`, a second `.tar.zst` for direct `roc run URL` bootstrap, and packagers for x64 and ARM64 on macOS, Windows, and Linux. Upload those assets together at the stamped URL after release verification.

## Verification

macOS is the development host for the repository, build pipeline, and portable tests. Validate launch, input, focus, DPI, Narrator, file dialogs, networking, and packaging on Windows. Run the packaged examples on clean x64 and ARM64 installations to confirm the app bundle needs no separate runtime. Before publishing native inputs, follow the [roc-automation platform guidance](https://github.com/lukewilliamboswell/roc-automation/blob/main/docs/platforms.adoc) for release provenance, locked inputs, and SBOMs.
