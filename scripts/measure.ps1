param(
    [string]$Exe = "$PSScriptRoot\..\target\release\replay-guardian.exe",
    [int]$Seconds = 600
)
$ErrorActionPreference = 'Stop'
$resolvedExe = (Resolve-Path -LiteralPath $Exe).Path
if (Get-Process -Name 'replay-guardian' -ErrorAction SilentlyContinue) {
    throw '请先退出已有 Replay Guardian，再运行测量，避免测到另一个实例。'
}
$outputDir = Join-Path $PSScriptRoot '..\docs\validation\local'
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
$testProcess = Start-Process -FilePath $resolvedExe -ArgumentList @('--safe', '--background') -WindowStyle Hidden -PassThru
try {
    Start-Sleep -Seconds 5
    $testProcess.Refresh()
    $startCpu = $testProcess.TotalProcessorTime.TotalSeconds
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $samples = [Collections.Generic.List[object]]::new()
    while ($clock.Elapsed.TotalSeconds -lt $Seconds) {
        $testProcess.Refresh()
        if ($testProcess.HasExited) { throw '测试进程意外退出' }
        $sample = [pscustomobject]@{
            Seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1)
            WorkingSetBytes = $testProcess.WorkingSet64
            PrivateBytes = $testProcess.PrivateMemorySize64
            CpuSeconds = $testProcess.TotalProcessorTime.TotalSeconds
            Handles = $testProcess.HandleCount
            Threads = $testProcess.Threads.Count
        }
        $samples.Add($sample)
        Write-Output ("{0:N0}s: working set {1:N2} MiB, handles {2}, threads {3}" -f $sample.Seconds, ($sample.WorkingSetBytes / 1MB), $sample.Handles, $sample.Threads)
        Start-Sleep -Seconds ([math]::Min(20, [math]::Max(1, $Seconds - $clock.Elapsed.TotalSeconds)))
    }
    $testProcess.Refresh()
    $elapsed = $clock.Elapsed.TotalSeconds
    $cpu = $testProcess.TotalProcessorTime.TotalSeconds - $startCpu
    $summary = [ordered]@{
        Mode = 'Auto, MCP active, --safe --background, settings never opened'
        ExeBytes = (Get-Item -LiteralPath $resolvedExe).Length
        Sha256 = (Get-FileHash -LiteralPath $resolvedExe -Algorithm SHA256).Hash
        Windows = [Environment]::OSVersion.VersionString
        DurationSeconds = [math]::Round($elapsed, 3)
        CpuSeconds = $cpu
        CpuPercentOneCore = [math]::Round(100 * $cpu / $elapsed, 6)
        CpuPercentMachine = [math]::Round(100 * $cpu / $elapsed / [Environment]::ProcessorCount, 6)
        WorkingSetMinBytes = ($samples | Measure-Object WorkingSetBytes -Minimum).Minimum
        WorkingSetMaxBytes = ($samples | Measure-Object WorkingSetBytes -Maximum).Maximum
        PrivateBytesMax = ($samples | Measure-Object PrivateBytes -Maximum).Maximum
        FirstHandles = $samples[0].Handles
        LastHandles = $samples[-1].Handles
        FirstThreads = $samples[0].Threads
        LastThreads = $samples[-1].Threads
    }
    $samples | Export-Csv -LiteralPath (Join-Path $outputDir 'performance-samples.csv') -NoTypeInformation -Encoding utf8
    $summary | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $outputDir 'performance.json') -Encoding utf8
    $summary | ConvertTo-Json
} finally {
    # Only the test process created above is stopped. Existing user instances are refused.
    if (!$testProcess.HasExited) { $testProcess.Kill(); $testProcess.WaitForExit() }
}
