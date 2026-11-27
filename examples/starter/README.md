# Reading list

Add a title, then select a row and use the toolbar to mark it read or remove it. Double-click a row or press Enter to toggle its read status. Click the Title header to switch between alphabetical and entry order. The list lives in memory for the current session.

- `src/` connects Win32 controls to Roc Signals.
- `logic/` is a Roc package with the reading-list decisions shared by the app and fuzz target.
- `assets/` holds the bitmap shown in the app.
- `windows.json` sets the MSI identity, product version, and publisher.
- `tests/specs/` contains a Win32 behaviour test and a recording demo in `.scm` files.
- `tests/fuzz/` checks reading-list invariants over generated titles.

Run the checks and packaging command from the app directory.

```sh
roc windows.roc -- test --check
roc windows.roc -- fuzz --check
roc windows.roc -- package --target x64mingw
```

On Windows, run `test` to execute the behaviour spec or `record tests/specs/demo.scm` for an animated GIF. The fuzz command runs on Apple Silicon macOS or x64 Linux. From the platform checkout, substitute `../../cli/windows.roc` for `windows.roc`.
