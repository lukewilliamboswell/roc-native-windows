param(
    [string] $Roc = "roc",
    [ValidateSet("x64mingw", "arm64mingw")] [string] $Target,
    [ValidateSet("counter", "starter", "todos", "files", "http", "workers")] [string[]] $Example,
    [string] $Starter,
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
        [Console]::Error.WriteLine("$Check accessibility check failed: $_")
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
    $Example = @("counter", "starter", "todos", "files", "http", "workers")
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$env:ROC_WINDOWS_ROC = $Roc
$failures = @()

foreach ($name in $Example) {
    $directory = Join-Path $repo "examples/$name"
    $tool = Join-Path $repo "cli/windows.roc"
    if ($Starter -and $name -eq "starter") {
        $directory = (Resolve-Path $Starter).Path
        $tool = Join-Path $directory "windows.roc"
    }
    Push-Location $directory
    try {
        & $Roc $tool -- build --target $Target
        if ($LASTEXITCODE -ne 0) { throw "Could not build $name for $Target." }
        $exe = @(Get-ChildItem -File "build/$Target" -Filter "*.exe")
        if ($exe.Count -ne 1) { throw "Expected exactly one built executable for $name in build/$Target." }
        $app = Start-Process -FilePath $exe[0].FullName -WorkingDirectory $directory -PassThru
        $probe = $null
        try {
            $probe = Start-Process -FilePath "powershell.exe" -ArgumentList @(
                "-NoProfile", "-Mta", "-File", ('"' + $PSCommandPath + '"'),
                "-AppPid", $app.Id, "-Check", $name
            ) -NoNewWindow -PassThru
            [void] $probe.Handle
            if (-not $probe.WaitForExit(45000)) { throw "$name's UI Automation client exceeded 45 seconds." }
            if ($probe.ExitCode -ne 0) { throw "$name's UI Automation client exited with code $($probe.ExitCode)." }
            Write-Output "PASS $name accessibility ($Target)"
        } finally {
            if ($probe -and -not $probe.HasExited) { $probe.Kill() }
            if (-not $app.HasExited) {
                [void] $app.CloseMainWindow()
                if (-not $app.WaitForExit(2000)) { $app.Kill() }
            }
        }
    } catch {
        $failures += "$name ($Target): $_"
    } finally {
        Pop-Location
    }
}

if ($failures.Count -gt 0) {
    throw "UI Automation verification failed:`n$($failures -join "`n")"
}
