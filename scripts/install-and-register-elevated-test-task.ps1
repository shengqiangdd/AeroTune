[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$InstallerPath = "F:\dev\cargo-tgt\aerotune-release-current\release\bundle\nsis\AeroTune_1.5.9_x64-setup.exe"
$ExpectedInstallerHash = "E4CD164809B9FE948E0F714682A9D6FAA0C48A6A5A526E558111C451F0A4CF28"
$TaskSetupScript = Join-Path $PSScriptRoot "elevated-test-task.ps1"
$TaskName = "AeroTune-ElevatedTest"
$LogPath = Join-Path $env:TEMP ("AeroTune-ElevatedTask-setup-" + [guid]::NewGuid().ToString("N") + ".log")
$ExitCode = 0

try {
    Start-Transcript -Path $LogPath -Force | Out-Null
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "This helper must already be running in an elevated PowerShell; it never requests or bypasses UAC itself."
    }
    if (Get-Process -Name "AeroTune" -ErrorAction SilentlyContinue) {
        throw "Close AeroTune normally before installation; this helper will not terminate it."
    }
    if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
        throw "The expected per-machine installer is missing: $InstallerPath"
    }
    $actualHash = (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash
    if ($actualHash -ne $ExpectedInstallerHash) {
        throw "Installer SHA-256 mismatch; refusing to execute an unexpected binary."
    }
    if (-not (Test-Path -LiteralPath $TaskSetupScript -PathType Leaf)) {
        throw "Task setup script is missing: $TaskSetupScript"
    }

    $service = New-Object -ComObject "Schedule.Service"
    $service.Connect()
    $folder = $service.GetFolder("\")
    try {
        $existingTask = $folder.GetTask($TaskName)
        if ($existingTask) { throw "Task $TaskName already exists; refusing to overwrite it." }
    } catch {
        if ($_.Exception.HResult -notin @(-2147216625, -2147024894)) { throw }
    }

    Write-Output "Starting the per-machine AeroTune installer."
    $installerProcess = Start-Process -FilePath $InstallerPath -Wait -PassThru
    if ($installerProcess.ExitCode -ne 0) {
        throw "Installer exited with code $($installerProcess.ExitCode); task was not registered."
    }

    $programFiles = [System.Environment]::GetFolderPath("ProgramFiles")
    $target = Join-Path $programFiles "AeroTune\AeroTune.exe"
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
        throw "Expected protected installation was not found at $target; task was not registered."
    }

    & $TaskSetupScript -Action Check -ExecutablePath $target
    & $TaskSetupScript -Action Register -ExecutablePath $target

    $task = $folder.GetTask($TaskName)
    $definition = $task.Definition
    if ($definition.Actions.Count -ne 1) { throw "Post-registration verification failed: expected one action." }
    if (-not ([System.IO.Path]::GetFullPath($definition.Actions.Item(1).Path)).Equals(
        [System.IO.Path]::GetFullPath($target),
        [System.StringComparison]::OrdinalIgnoreCase
    )) { throw "Post-registration verification failed: task action path mismatch." }
    if ($definition.Actions.Item(1).Arguments) { throw "Post-registration verification failed: task has arguments." }
    if ($definition.Triggers.Count -ne 0) { throw "Post-registration verification failed: task has an automatic trigger." }
    if (($definition.Principal.LogonType -ne 3) -or ($definition.Principal.RunLevel -ne 1)) {
        throw "Post-registration verification failed: wrong logon type or run level."
    }
    $taskUserId = [string]$definition.Principal.UserId
    try {
        if ($taskUserId -match "^S-\d-") {
            $taskUserSid = [System.Security.Principal.SecurityIdentifier]::new($taskUserId).Value
        } else {
            $taskUserSid = ([System.Security.Principal.NTAccount]::new($taskUserId)).Translate(
                [System.Security.Principal.SecurityIdentifier]
            ).Value
        }
    } catch {
        throw "Post-registration verification failed: task principal could not be resolved to a SID."
    }
    if ($taskUserSid -ne $identity.User.Value) {
        throw "Post-registration verification failed: task principal is not the registering user."
    }
    if (-not $definition.Settings.AllowDemandStart) { throw "Post-registration verification failed: demand start is disabled." }

    $expectedUserSid = $identity.User.Value
    $taskDacl = $task.GetSecurityDescriptor(4)
    $parsedDacl = [System.Security.AccessControl.RawSecurityDescriptor]::new($taskDacl)
    $userMask = 0
    foreach ($ace in $parsedDacl.DiscretionaryAcl) {
        if (($ace.SecurityIdentifier.Value -eq $expectedUserSid) -and
            ($ace.AceQualifier -eq [System.Security.AccessControl.AceQualifier]::AccessAllowed)) {
            $userMask = $userMask -bor [int]$ace.AccessMask
        }
    }
    $readExecuteMask = [int](
        [System.Security.AccessControl.FileSystemRights]::ReadAndExecute -bor
        [System.Security.AccessControl.FileSystemRights]::Synchronize
    )
    $writeMask = [int](
        [System.Security.AccessControl.FileSystemRights]::Write -bor
        [System.Security.AccessControl.FileSystemRights]::Delete -bor
        [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
        [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
        [System.Security.AccessControl.FileSystemRights]::TakeOwnership -bor
        [int]0x10000000 -bor
        [int]0x40000000
    )
    if (($userMask -band $readExecuteMask) -ne $readExecuteMask) {
        throw "Post-registration verification failed: current user lacks read/execute task rights."
    }
    if (($userMask -band $writeMask) -ne 0) {
        throw "Post-registration verification failed: current user has task write rights."
    }
    if (($userMask -band (-bnot $readExecuteMask)) -ne 0) {
        throw "Post-registration verification failed: current user has unexpected task rights."
    }

    Write-Output "Task registration and read-only configuration checks passed."
    Write-Output "Task: $TaskName"
    Write-Output "Target: $target"
    Write-Output "Triggers: 0; run level: highest; user ACE: read/execute only."
} catch {
    Write-Output ("ERROR: " + $_.Exception.Message)
    $ExitCode = 1
} finally {
    try { Stop-Transcript | Out-Null } catch { }
}

Write-Output "Setup log: $LogPath"
exit $ExitCode
