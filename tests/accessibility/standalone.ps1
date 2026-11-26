param(
    [Parameter(Mandatory)] [string] $ReleaseDirectory,
    [string] $Roc = "roc"
)

$ErrorActionPreference = "Stop"
$release = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$tools = @(Get-ChildItem -LiteralPath $release -File -Filter 'windows-*.tar.zst')
if ($tools.Count -ne 1) { throw "Expected one standalone tool bundle in $release." }
$compiler = (Get-Command $Roc -CommandType Application).Source
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$scratch = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) "roc-windows-preview-$([Guid]::NewGuid())")
$serverBinary = Join-Path $scratch.FullName 'serve.exe'
$serverLog = Join-Path $scratch.FullName 'serve.log'
$serverErrors = Join-Path $scratch.FullName 'serve-errors.log'
$project = Join-Path $scratch.FullName 'reading-list'

Push-Location -LiteralPath $repo
try {
    & $compiler build scripts/serve.roc "--output=$serverBinary"
    if ($LASTEXITCODE -ne 0) { throw 'Could not build the release preview server.' }
} finally {
    Pop-Location
}

$server = Start-Process -FilePath $serverBinary -ArgumentList '.' -WorkingDirectory $release `
    -WindowStyle Hidden -RedirectStandardOutput $serverLog -RedirectStandardError $serverErrors -PassThru
$originalCache = $env:ROC_CACHE_DIR
try {
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($server.HasExited -or $deadline.Elapsed.TotalSeconds -ge 10) {
            Get-Content -LiteralPath $serverLog, $serverErrors
            throw 'The release preview server did not start.'
        }
        Start-Sleep -Milliseconds 100
    } until (Select-String -LiteralPath $serverLog -SimpleMatch 'at http://127.0.0.1:8000/' -Quiet)

    $env:ROC_CACHE_DIR = Join-Path $scratch.FullName 'roc-cache'
    Push-Location -LiteralPath $scratch.FullName
    try {
        & $compiler "http://127.0.0.1:8000/$($tools[0].Name)" -- init $project
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
} finally {
    $env:ROC_CACHE_DIR = $originalCache
    if (!$server.HasExited) {
        $server.Kill()
        [void] $server.WaitForExit(2000)
    }
}
