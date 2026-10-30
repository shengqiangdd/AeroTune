param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [int]$DurationSeconds = 60,
    [int]$WarmupSeconds = 15
)

$process = Get-Process -Id $ProcessId -ErrorAction Stop
$processPath = $process.Path
Start-Sleep -Seconds $WarmupSeconds
$coreCount = [Environment]::ProcessorCount
$cpuSamples = @()
$wsSamples = @()
$privateSamples = @()
for ($i = 0; $i -lt $DurationSeconds; $i += 10) {
    $sampleWindow = [Math]::Min(10, $DurationSeconds - $i)
    $process = Get-Process -Id $ProcessId -ErrorAction Stop
    $cpuBefore = $process.CPU
    $sampleStart = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Seconds $sampleWindow
    $process = Get-Process -Id $ProcessId -ErrorAction Stop
    $sampleSeconds = $sampleStart.Elapsed.TotalSeconds
    $cpuSamples += 100 * ($process.CPU - $cpuBefore) / ($sampleSeconds * $coreCount)
    $wsSamples += $process.WorkingSet64 / 1MB
    $privateSamples += $process.PrivateMemorySize64 / 1MB
}
[pscustomobject]@{
    PID = $ProcessId
    Path = $processPath
    DurationSeconds = $DurationSeconds
    AverageMachineCpuPercent = [math]::Round(($cpuSamples | Measure-Object -Average).Average, 3)
    MedianWindowCpuPercent = [math]::Round(($cpuSamples | Sort-Object)[[Math]::Floor($cpuSamples.Count / 2)], 3)
    MaxWindowCpuPercent = [math]::Round(($cpuSamples | Measure-Object -Maximum).Maximum, 3)
    WorkingSetMedianMiB = [math]::Round(($wsSamples | Sort-Object)[[Math]::Floor($wsSamples.Count / 2)], 1)
    PrivateMedianMiB = [math]::Round(($privateSamples | Sort-Object)[[Math]::Floor($privateSamples.Count / 2)], 1)
    LogicalCores = $coreCount
    Responding = $process.Responding
} | Format-List
