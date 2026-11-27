$ErrorActionPreference = 'Stop'

if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:PROCESSOR_ARCHITECTURE -ne 'ARM64') {
    throw 'Desktop preparation requires an ephemeral GitHub-hosted ARM64 Windows runner.'
}
$session = [Diagnostics.Process]::GetCurrentProcess().SessionId
if ($session -eq 0 -or -not [Environment]::UserInteractive) {
    throw 'Desktop preparation requires an interactive Windows session.'
}

# The ARM64 image can leave its first sign-in privacy window over the desktop.
# https://github.com/actions/runner-images/issues/14069
$policy = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OOBE'
New-Item -Path $policy -Force | Out-Null
New-ItemProperty -Path $policy -Name DisablePrivacyExperience -PropertyType DWord -Value 1 -Force | Out-Null

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class RocHostedDesktop {
    public static uint ForegroundProcessId() {
        uint processId;
        GetWindowThreadProcessId(GetForegroundWindow(), out processId);
        return processId;
    }

    public static uint DismissStartMenu(uint processId) {
        if (ForegroundProcessId() != processId) return 0;
        var keyUp = EscapeKey(2); // KEYEVENTF_KEYUP
        var size = Marshal.SizeOf(typeof(Input));
        var sent = SendInput(2, new[] { EscapeKey(0), keyUp }, size);
        if (sent == 1) SendInput(1, new[] { keyUp }, size);
        return sent;
    }

    private static Input EscapeKey(uint flags) {
        return new Input { Type = 1, Data = new InputData {
            Keyboard = new KeyboardInput { Key = 0x1b, Flags = flags }
        } };
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Input { public uint Type; public InputData Data; }

    [StructLayout(LayoutKind.Explicit)]
    private struct InputData {
        [FieldOffset(0)] public KeyboardInput Keyboard;
        [FieldOffset(0)] public MouseInput Mouse; // Preserve the native INPUT union size.
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct KeyboardInput { public ushort Key, Scan; public uint Flags, Time; public UIntPtr Extra; }

    [StructLayout(LayoutKind.Sequential)]
    private struct MouseInput { public int X, Y; public uint Data, Flags, Time; public UIntPtr Extra; }

    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
    [DllImport("user32.dll")] private static extern uint SendInput(uint count, [In] Input[] inputs, int size);
}
'@

$deadline = [Diagnostics.Stopwatch]::StartNew()
$clear = [Diagnostics.Stopwatch]::StartNew()
while ($deadline.Elapsed.TotalSeconds -lt 30) {
    $foregroundId = [RocHostedDesktop]::ForegroundProcessId()
    $foreground = if ($foregroundId) { Get-Process -Id $foregroundId -ErrorAction SilentlyContinue }
    if ($foreground -and $foreground.SessionId -eq $session) {
        if ($foreground.ProcessName -eq 'WWAHost') {
            $clear.Restart()
            [void] $foreground.Handle
            if ([RocHostedDesktop]::ForegroundProcessId() -eq $foreground.Id -and -not $foreground.HasExited) {
                Write-Output "Closing the first sign-in privacy window (WWAHost, PID $foregroundId)."
                $foreground.Kill()
                [void] $foreground.WaitForExit(3000)
            }
        } elseif ($foreground.ProcessName -in @('StartMenuExperienceHost', 'SearchHost')) {
            $clear.Restart()
            Write-Output "Dismissing the foreground $($foreground.ProcessName) window (PID $foregroundId)."
            [void] [RocHostedDesktop]::DismissStartMenu($foreground.Id)
        }
    }
    if ($clear.Elapsed.TotalSeconds -ge 3) {
        Write-Output 'The ARM64 runner desktop is clear of first sign-in, Start, and Search windows.'
        return
    }
    Start-Sleep -Milliseconds 500
}
throw 'The first sign-in, Start, or Search window still owns the ARM64 runner foreground.'
