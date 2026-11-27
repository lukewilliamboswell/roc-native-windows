param(
    [Parameter(Mandatory)] [string] $PackageDirectory,
    [Parameter(Mandatory)] [ValidateSet('x64mingw', 'arm64mingw')] [string] $Target
)

$ErrorActionPreference = 'Stop'
$packages = (Resolve-Path -LiteralPath $PackageDirectory).Path
$scratch = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) "roc-windows-msi-$([Guid]::NewGuid())")
$installDirectory = Join-Path $scratch.FullName 'installed'
$installedExe = Join-Path $installDirectory 'reading-list.exe'
$installedMsi = $null

function Invoke-Installer($operation, $msi, $log, $destination) {
    $arguments = @($operation, ('"{0}"' -f $msi), '/qn', '/norestart', '/l*v', ('"{0}"' -f $log))
    if ($destination) { $arguments += ('INSTALLDIR="{0}"' -f $destination) }
    $process = Start-Process msiexec.exe -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Tail 100 | Write-Host }
        throw "Windows Installer $operation exited with code $($process.ExitCode)."
    }
}

try {
    foreach ($version in @('1.0.0', '1.0.1')) {
        $msi = Join-Path $packages "reading-list-$Target-$version.msi"
        $digestFile = Join-Path $packages "reading-list-$Target-$version.exe.sha256"
        $expected = (Get-Content -LiteralPath $digestFile -Raw).Trim()
        Invoke-Installer '/i' $msi (Join-Path $scratch.FullName "install-$version.log") $installDirectory
        $installedMsi = $msi
        if ((Get-FileHash -LiteralPath $installedExe).Hash -ne $expected) {
            throw "Installed Reading List $version differs from the built executable."
        }
        $app = Start-Process -FilePath $installedExe -WorkingDirectory $installDirectory -PassThru
        try {
            if (!$app.WaitForInputIdle(10000) -or $app.HasExited) { throw "Installed Reading List $version failed to start." }
        } finally {
            if (!$app.HasExited -and (!$app.CloseMainWindow() -or !$app.WaitForExit(2000))) {
                $app.Kill()
                [void] $app.WaitForExit(2000)
            }
        }
    }
} finally {
    if ($installedMsi) {
        Invoke-Installer '/x' $installedMsi (Join-Path $scratch.FullName 'uninstall.log')
    }
}
if (Test-Path -LiteralPath $installedExe) { throw 'Uninstall left the Reading List executable in place.' }
Write-Output "PASS Reading List MSI install, upgrade, launch, and uninstall ($Target)."
