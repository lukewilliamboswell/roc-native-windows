param([Parameter(Mandatory)] [string] $Directory)

$ErrorActionPreference = "Stop"
$staged = (Resolve-Path -LiteralPath $Directory).Path
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$planDirs = @(Get-ChildItem -LiteralPath $staged -Recurse -Directory -Filter "plans")
if ($planDirs.Count -eq 0) { throw "No scenario plans were staged in $staged." }
$output = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) "roc-windows-scenarios-$([Guid]::NewGuid())")
$failures = 0

foreach ($planDir in $planDirs) {
    $app = $planDir.Parent
    $relative = $app.FullName.Substring($staged.Length).TrimStart('\', '/')
    $executables = @(Get-ChildItem -LiteralPath $app.FullName -File -Filter "*.exe")
    $plans = @(Get-ChildItem -LiteralPath $planDir.FullName -File -Filter "*.scm.plan" | Sort-Object Name)
    if ($executables.Count -ne 1 -or $plans.Count -eq 0) {
        throw "Expected an executable and scenario plans for $relative."
    }
    foreach ($plan in $plans) {
        $scenario = $plan.BaseName
        $scenarioOut = New-Item -ItemType Directory -Path (Join-Path $output.FullName "$relative/$scenario") -Force
        $scenarioPlan = Join-Path $scenarioOut.FullName "plan.bin"
        Copy-Item -LiteralPath $plan.FullName -Destination $scenarioPlan
        $start = [Diagnostics.ProcessStartInfo]::new($executables[0].FullName)
        $start.WorkingDirectory = Join-Path $repo $relative
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.EnvironmentVariables["ROC_WINDOWS_SPEC"] = $scenarioPlan
        $start.EnvironmentVariables["ROC_WINDOWS_SPEC_OUT"] = $scenarioOut.FullName
        $start.EnvironmentVariables["ROC_WINDOWS_SPEC_RECORD"] = ""
        $start.EnvironmentVariables["ROC_WINDOWS_SPEC_FAILURE_SCREENSHOT"] = ""
        $process = [Diagnostics.Process]::Start($start)
        try {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(120000)) {
                $process.Kill()
                throw "Native scenario exceeded 120 seconds."
            }
            $observed = $stdout.GetAwaiter().GetResult().Trim()
            $marker = ($observed -split "`n")[-1].Trim()
            if ($process.ExitCode -ne 0 -or $marker -cne "roc-windows-spec 1 done") {
                $errors = $stderr.GetAwaiter().GetResult().Trim()
                throw "Native scenario exited with code $($process.ExitCode). $errors $observed"
            }
            Write-Output "PASS $relative/$scenario"
        } catch {
            $failures++
            Write-Output "FAIL $relative/${scenario}: $_ (artifacts in $($scenarioOut.FullName))"
        } finally {
            $process.Dispose()
        }
    }
}
if ($failures -gt 0) { throw "$failures staged native scenarios failed." }
Write-Output "Passed staged Win32 scenarios for $($planDirs.Count) apps."
