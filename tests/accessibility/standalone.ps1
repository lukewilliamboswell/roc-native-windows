[CmdletBinding(DefaultParameterSetName = 'Preview')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Preview')] [string] $ReleaseDirectory,
    [Parameter(Mandatory, ParameterSetName = 'Published')] [uri] $ReleaseUrl,
    [string] $Roc = "roc",
    [switch] $TestInstaller,
    [string] $ArmInstallersDirectory
)

$ErrorActionPreference = "Stop"
$compiler = (Get-Command $Roc -CommandType Application).Source
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$scratch = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) "roc-windows-preview-$([Guid]::NewGuid())")
$project = Join-Path $scratch.FullName 'reading-list'
$server = $null

function New-ReadingListInstallers($target, $destination) {
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    $destination = (Resolve-Path -LiteralPath $destination).Path
    $identity = "com.example.reading-list.$([Guid]::NewGuid().ToString('N'))"
    Push-Location -LiteralPath $project
    try {
        foreach ($version in @('1.0.0', '1.0.1')) {
            & $compiler windows.roc -- package --target $target --profile size --id $identity --app-version $version
            if ($LASTEXITCODE -ne 0) { throw "Could not package Reading List $version for $target." }
            Move-Item -LiteralPath "dist/reading-list-$target.msi" -Destination (Join-Path $destination "reading-list-$target-$version.msi")
            (Get-FileHash -LiteralPath "build/$target/reading-list.exe").Hash |
                Set-Content -LiteralPath (Join-Path $destination "reading-list-$target-$version.exe.sha256") -Encoding Ascii
        }
    } finally {
        Pop-Location
    }
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
    if ($TestInstaller) {
        $installers = Join-Path $scratch.FullName 'x64-installers'
        New-ReadingListInstallers x64mingw $installers
        & (Join-Path $PSScriptRoot 'installer.ps1') -PackageDirectory $installers -Target x64mingw
    }
    if ($ArmInstallersDirectory) { New-ReadingListInstallers arm64mingw $ArmInstallersDirectory }
} finally {
    $env:ROC_CACHE_DIR = $originalCache
    if ($server -and !$server.HasExited) {
        $server.Kill()
        [void] $server.WaitForExit(2000)
    }
}
