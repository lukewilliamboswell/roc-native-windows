param(
    [string] $Roc = "roc",
    [ValidateSet("x64mingw", "arm64mingw")] [string] $Target,
    [ValidateSet("speed", "size")] [string] $BuildProfile = "speed",
    [ValidateSet("counter", "starter", "todos", "files", "http", "workers", "progress")] [string[]] $Example,
    [string] $Starter,
    [switch] $FreshStarter,
    [string] $BuiltApps,
    [int] $AppPid,
    [string] $Check
)

$ErrorActionPreference = "Stop"
if ($AppPid -gt 0) {
    try {
        Add-Type -AssemblyName "UIAutomationClient", "UIAutomationTypes", "WindowsBase"
        $references = @(
            "System.dll", "System.Core.dll",
            [System.Windows.Automation.AutomationElement].Assembly.Location,
            [System.Windows.Automation.AutomationElementIdentifiers].Assembly.Location,
            [System.Windows.Rect].Assembly.Location
        )
        Add-Type -Path (Join-Path $PSScriptRoot "Probe.cs") -ReferencedAssemblies $references
        [RocWindowsAccessibility]::Check([Diagnostics.Process]::GetProcessById($AppPid), $Check)
        exit 0
    } catch {
        [Console]::Error.WriteLine("$Check accessibility check failed.")
        [Console]::Error.WriteLine($_.Exception.ToString())
        exit 1
    }
}

if (-not [Environment]::UserInteractive -or [Diagnostics.Process]::GetCurrentProcess().SessionId -eq 0) {
    throw "An interactive Windows desktop is required for UI Automation verification."
}
if (-not $Target) {
    $Target = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64" -or $env:PROCESSOR_ARCHITEW6432 -eq "ARM64") { "arm64mingw" } else { "x64mingw" }
}
if (-not $Example) {
    $Example = @("counter", "starter", "todos", "files", "http", "workers", "progress")
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
if ($BuiltApps) {
    if ($Starter) { throw "-Starter and -BuiltApps cannot be combined." }
    $BuiltApps = (Resolve-Path -LiteralPath $BuiltApps).Path
}
$env:ROC_WINDOWS_ROC = $Roc
$windows = Get-ItemProperty -LiteralPath "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion"
Write-Output "Windows build $($windows.CurrentBuild).$($windows.UBR), app target $Target"

function Invoke-ExampleProbe($name, $directory, $tool, $label, $builtDirectory) {
    Push-Location $directory
    try {
        if (-not $builtDirectory) {
            & $Roc $tool -- build --target $Target --profile $BuildProfile
            if ($LASTEXITCODE -ne 0) { throw "Could not build $label for $Target." }
            $builtDirectory = "build/$Target"
        }
        $exe = @(Get-ChildItem -LiteralPath $builtDirectory -File -Filter "*.exe")
        if ($exe.Count -ne 1) { throw "Expected exactly one built executable for $label in $builtDirectory." }
        $app = Start-Process -FilePath $exe[0].FullName -WorkingDirectory $directory -PassThru
        $probe = $null
        try {
            $probe = Start-Process -FilePath "powershell.exe" -ArgumentList @(
                "-NoProfile", "-Mta", "-File", ('"' + $PSCommandPath + '"'),
                "-AppPid", $app.Id, "-Check", $name
            ) -WorkingDirectory $directory -NoNewWindow -PassThru
            [void] $probe.Handle
            if (-not $probe.WaitForExit(45000)) { throw "$label UI Automation client exceeded 45 seconds." }
            if ($probe.ExitCode -ne 0) { throw "$label UI Automation client exited with code $($probe.ExitCode)." }
            Write-Output "PASS $label accessibility ($Target)"
        } finally {
            if ($probe -and -not $probe.HasExited) {
                $probe.Kill()
                [void] $probe.WaitForExit(2000)
            }
            if (-not $app.HasExited) {
                [void] $app.CloseMainWindow()
                if (-not $app.WaitForExit(2000)) {
                    $app.Kill()
                    [void] $app.WaitForExit(2000)
                }
            }
        }
    } finally {
        Pop-Location
    }
}

$launcher = Join-Path $repo "cli/windows.roc"
$failures = @()
foreach ($name in $Example) {
    try {
        if ($BuiltApps) {
            $directory = Join-Path $BuiltApps $name
            Invoke-ExampleProbe $name $directory $null $name $directory
        } elseif ($Starter -and $name -eq "starter") {
            $directory = (Resolve-Path $Starter).Path
            Invoke-ExampleProbe $name $directory (Join-Path $directory "windows.roc") "supplied starter"
        } elseif ($name -eq "progress") {
            Invoke-ExampleProbe $name (Join-Path $PSScriptRoot "progress") $launcher "determinate progress fixture"
        } else {
            Invoke-ExampleProbe $name (Join-Path $repo "examples/$name") $launcher $name
        }
    } catch {
        $failures += "$name ($Target): $_"
    }
}

if ($FreshStarter) {
    $scratch = Join-Path $repo "build/accessibility-$([Guid]::NewGuid().ToString('N'))"
    $directory = Join-Path $scratch "reading-list"
    try {
        if ($BuiltApps) {
            $directory = Join-Path $BuiltApps "fresh-starter"
            Invoke-ExampleProbe "starter" $directory $null "fresh starter" $directory
        } else {
            Push-Location $repo
            try {
                & $Roc $launcher -- init --platform platform/main.roc $directory
                if ($LASTEXITCODE -ne 0) { throw "Could not initialize the fresh Reading List project." }
            } finally {
                Pop-Location
            }
            Invoke-ExampleProbe "starter" $directory (Join-Path $directory "windows.roc") "fresh starter"
        }
    } catch {
        $failures += "fresh starter ($Target): $_"
    } finally {
        if (Test-Path -LiteralPath $scratch) { Remove-Item -LiteralPath $scratch -Recurse -Force }
    }
}

if ($failures.Count -gt 0) {
    throw "UI Automation verification failed:`n$($failures -join "`n")"
}
