# Native input fixture

This fixture exercises mouse capture, paced and ordinary movement, navigation
keys, mount retirement, and the native trackbar through the behavior runner.
The bitmap gives each input surface a stable client area.

From the repository root, prepare the host with `blueprint run bundle`. Then
run the fixture from its app directory on a Windows desktop.

```sh
cd tests/native-input
roc ../../cli/windows.roc -- test
```

Use `test --check` on macOS or Linux to check the Roc source and spec syntax.
