# Reading List keyboard test

The test drives the Starter example through the Windows keyboard message loop
and reads the focused HWND from the app's UI thread. It checks forward and
reverse Tab order, focus recovery when Add disables itself, and automatic
scrolling to reveal focus in a narrow window. It also opens the native
Status filter and verifies Enter and Escape close it. Run from a Windows desktop where
the probe can send keyboard input to the app at the same privilege level.

From the repository root, use PowerShell with Roc, Zig, and the built local
platform. The test build uses the executable name as the window title.

```powershell
New-Item -ItemType Directory -Force build/native-keyboard/assets
roc build examples/starter/src/main.roc --target=x64mingw --output=build/native-keyboard/reading-list.exe
Copy-Item examples/starter/assets/* build/native-keyboard/assets -Recurse -Force
zig cc -target x86_64-windows-gnu -O2 tests/native-keyboard/reading-list.c -luser32 -o build/native-keyboard/probe.exe
$app = Start-Process build/native-keyboard/reading-list.exe -PassThru
try {
    & build/native-keyboard/probe.exe
    if ($LASTEXITCODE -ne 0) { throw "Keyboard probe failed with exit code $LASTEXITCODE" }
} finally {
    if (!$app.WaitForExit(5000)) { $app.Kill() }
}
```

For native ARM64 use Roc's `arm64mingw` and Zig's `aarch64-windows-gnu` targets.

## Todos calendar

The calendar probe uses the same keyboard path to open the owned date chooser,
navigate across a leap-day boundary, verify a single Tab stop for the days,
and check that selecting and cancelling both return focus to the opener.

```powershell
roc build examples/todos/src/main.roc --target=x64mingw --output=build/native-keyboard/todos.exe
Copy-Item examples/todos/assets/* build/native-keyboard/assets -Recurse -Force
zig cc -target x86_64-windows-gnu -O2 tests/native-keyboard/calendar.c -luser32 -o build/native-keyboard/calendar.exe
$app = Start-Process build/native-keyboard/todos.exe -PassThru
try {
    & build/native-keyboard/calendar.exe
    if ($LASTEXITCODE -ne 0) { throw "Calendar probe failed with exit code $LASTEXITCODE" }
} finally {
    if (!$app.WaitForExit(5000)) { $app.Kill() }
}
```
