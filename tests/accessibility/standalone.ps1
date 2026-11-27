[CmdletBinding(DefaultParameterSetName = 'Preview')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Preview')] [string] $ReleaseDirectory,
    [Parameter(Mandatory, ParameterSetName = 'Published')] [uri] $ReleaseUrl,
    [string] $Roc = "roc",
    [switch] $TestInstaller
)

$ErrorActionPreference = "Stop"
$compiler = (Get-Command $Roc -CommandType Application).Source
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$scratch = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) "roc-windows-preview-$([Guid]::NewGuid())")
$project = Join-Path $scratch.FullName 'reading-list'
$server = $null

function Invoke-Installer($operation, $msi, $log, $installDirectory) {
    $arguments = @($operation, ('"{0}"' -f $msi), '/qn', '/norestart', '/l*v', ('"{0}"' -f $log))
    if ($installDirectory) { $arguments += ('INSTALLDIR="{0}"' -f $installDirectory) }
    $process = Start-Process msiexec.exe -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        if (Test-Path -LiteralPath $log) { Get-Content -LiteralPath $log -Tail 100 | Write-Host }
        throw "Windows Installer $operation exited with code $($process.ExitCode)."
    }
}

function Test-ReadingListInstaller {
    $installDirectory = Join-Path $scratch.FullName 'installed'
    $identity = "com.example.reading-list.$([Guid]::NewGuid().ToString('N'))"
    $localMsi = Join-Path $project 'dist/reading-list-x64mingw.msi'
    $installedMsi = $null
    Push-Location -LiteralPath $project
    try {
        foreach ($version in @('1.0.0', '1.0.1')) {
            & $compiler windows.roc -- package --target x64mingw --profile size --id $identity --app-version $version
            if ($LASTEXITCODE -ne 0) { throw "Could not package Reading List $version." }
            $msi = Join-Path $scratch.FullName "reading-list-$version.msi"
            Move-Item -LiteralPath $localMsi -Destination $msi
            Invoke-Installer '/i' $msi (Join-Path $scratch.FullName "install-$version.log") $installDirectory
            $installedMsi = $msi
            $installedExe = Join-Path $installDirectory 'reading-list.exe'
            $expected = Get-FileHash -LiteralPath 'build/x64mingw/reading-list.exe'
            $actual = Get-FileHash -LiteralPath $installedExe
            if ($actual.Hash -ne $expected.Hash) { throw "Installed Reading List $version differs from the built executable." }
        }
        $app = Start-Process -FilePath $installedExe -WorkingDirectory $installDirectory -PassThru
        try {
            if (!$app.WaitForInputIdle(10000) -or $app.HasExited) { throw 'Installed Reading List failed to start.' }
        } finally {
            if (!$app.HasExited -and (!$app.CloseMainWindow() -or !$app.WaitForExit(2000))) {
                $app.Kill()
                [void] $app.WaitForExit(2000)
            }
        }
    } finally {
        Pop-Location
        if ($installedMsi) {
            Invoke-Installer '/x' $installedMsi (Join-Path $scratch.FullName 'uninstall.log')
        }
    }
    if (Test-Path -LiteralPath $installedExe) { throw 'Uninstall left the Reading List executable in place.' }
    Write-Output 'PASS Reading List MSI install, upgrade, launch, and uninstall.'
}

$originalCache = $env:ROC_CACHE_DIR
try {
    if ($ReleaseDirectory) {
        $release = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
        $tools = @(Get-ChildItem -LiteralPath $release -File -Filter 'windows-*.tar.zst')
        if ($tools.Count -ne 1) { throw "Expected one standalone tool bundle in $release." }
        $serverBinary = Join-Path $scratch.FullName 'serve.exe'
        $serverLog = Join-Path $scratch.FullName 'serve.log'
        $serverErrors = Join-Path $scratch.FullName 'serve-errors.log'
        Push-Location -LiteralPath $repo
        try {
            & $compiler build scripts/serve.roc "--output=$serverBinary"
            if ($LASTEXITCODE -ne 0) { throw 'Could not build the release preview server.' }
        } finally {
            Pop-Location
        }
        $server = Start-Process -FilePath $serverBinary -ArgumentList '.' -WorkingDirectory $release `
            -WindowStyle Hidden -RedirectStandardOutput $serverLog -RedirectStandardError $serverErrors -PassThru
        $deadline = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ($server.HasExited -or $deadline.Elapsed.TotalSeconds -ge 10) {
                Get-Content -LiteralPath $serverLog, $serverErrors
                throw 'The release preview server did not start.'
            }
            Start-Sleep -Milliseconds 100
        } until (Select-String -LiteralPath $serverLog -SimpleMatch 'at http://127.0.0.1:8000/' -Quiet)
        $ReleaseUrl = "http://127.0.0.1:8000/$($tools[0].Name)"
    }

    $env:ROC_CACHE_DIR = Join-Path $scratch.FullName 'roc-cache'
    Push-Location -LiteralPath $scratch.FullName
    try {
        & $compiler $ReleaseUrl.AbsoluteUri -- init $project
        if ($LASTEXITCODE -ne 0) { throw 'The standalone launcher could not generate Reading List.' }
    } finally {
        Pop-Location
    }

    Push-Location -LiteralPath $project
    try {
        & $compiler windows.roc -- test --target x64mingw
        if ($LASTEXITCODE -ne 0) { throw 'The standalone Reading List scenarios failed.' }
    } finally {
        Pop-Location
    }
    & (Join-Path $PSScriptRoot 'run.ps1') -Roc $compiler -Target x64mingw -Example starter -Starter $project
    if ($TestInstaller) { Test-ReadingListInstaller }
} finally {
    $env:ROC_CACHE_DIR = $originalCache
    if ($server -and !$server.HasExited) {
        $server.Kill()
        [void] $server.WaitForExit(2000)
    }
}
