[CmdletBinding()]
param(
    [ValidateSet("Check", "Register", "Start", "Remove")]
    [string]$Action = "Check",
    [string]$ExecutablePath = (Join-Path $env:ProgramFiles "AeroTune\AeroTune.exe")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$TaskName = "AeroTune-ElevatedTest"
$TaskDescription = "AeroTune manual-only elevated test launcher; no logon or startup trigger."

function Test-AdministratorToken {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-AdministratorToken {
    if (-not (Test-AdministratorToken)) {
        throw "Register/Remove require one explicit administrator consent. Manually open an elevated PowerShell when available; this script never launches or bypasses UAC."
    }
}

function Resolve-ProtectedExecutable {
    param([Parameter(Mandatory = $true)][string]$Candidate)

    if (-not (Test-Path -LiteralPath $Candidate -PathType Leaf)) {
        throw "AeroTune executable not found: $Candidate. Install with the per-machine installer under Program Files first."
    }

    $exe = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Candidate).Path)
    $programFiles = [System.IO.Path]::GetFullPath([System.Environment]::GetFolderPath("ProgramFiles"))
    $prefix = $programFiles.TrimEnd([char]92) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $exe.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Registration refused: Target must be under protected Program Files; current path is $exe"
    }
    if (-not [System.IO.Path]::GetFileName($exe).Equals("AeroTune.exe", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Registration refused: Only AeroTune.exe may be used as the elevated test target."
    }
    $installDirectory = Split-Path -Parent $exe
    if (-not (Split-Path -Leaf $installDirectory).Equals("AeroTune", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Registration refused: AeroTune.exe must be in the protected Program Files/AeroTune directory."
    }

    $trustedInstallerSid = ""
    try {
        $trustedInstallerSid = ([System.Security.Principal.NTAccount]::new("NT SERVICE", "TrustedInstaller")).Translate([System.Security.Principal.SecurityIdentifier]).Value
    } catch {
        # Some Windows editions do not resolve the service account name; other ACL checks still apply.
    }
    $allowedOwnerSids = @("S-1-5-18", "S-1-5-32-544")
    if ($trustedInstallerSid) { $allowedOwnerSids += $trustedInstallerSid }

    $trustedWriterSids = @("S-1-5-18", "S-1-5-32-544")
    if ($trustedInstallerSid) { $trustedWriterSids += $trustedInstallerSid }
    $creatorOwnerSid = "S-1-3-0"
    $writeMask = [int](
        [System.Security.AccessControl.FileSystemRights]::Write -bor
        [System.Security.AccessControl.FileSystemRights]::Delete -bor
        [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
        [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
        [System.Security.AccessControl.FileSystemRights]::TakeOwnership -bor
        [int]0x10000000 -bor # GENERIC_ALL
        [int]0x40000000      # GENERIC_WRITE
    )

    $pathsToCheck = [System.Collections.Generic.List[string]]::new()
    $pathsToCheck.Add($exe)
    $cursor = Split-Path -Parent $exe
    while ($true) {
        $pathsToCheck.Add($cursor)
        if ($cursor.TrimEnd([char]92).Equals(
            $programFiles.TrimEnd([char]92),
            [System.StringComparison]::OrdinalIgnoreCase
        )) { break }
        if (-not $cursor.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Registration refused: Unexpected path component between Program Files and target: $cursor"
        }
        $parent = Split-Path -Parent $cursor
        if (-not $parent -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Unable to validate install-path ACL: $cursor"
        }
        $cursor = $parent
    }

    foreach ($path in $pathsToCheck) {
        $parentItem = Get-Item -LiteralPath $path -Force -ErrorAction Stop
        if (($parentItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Registration refused: Install path contains a reparse point or symlink: $path"
        }
    }
    $installDirectory = Split-Path -Parent $exe
    $pendingDirectories = [System.Collections.Generic.Stack[string]]::new()
    $pendingDirectories.Push($installDirectory)
    while ($pendingDirectories.Count -gt 0) {
        $directory = $pendingDirectories.Pop()
        foreach ($entry in Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop) {
            $pathsToCheck.Add($entry.FullName)
            if ($entry.PSIsContainer) {
                if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "Registration refused: Install tree contains a reparse point or symlink: $($entry.FullName)"
                }
                $pendingDirectories.Push($entry.FullName)
            }
        }
    }

    foreach ($path in $pathsToCheck) {
        $item = Get-Item -LiteralPath $path -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Registration refused: Target path contains a reparse point or symlink: $path"
        }
        $acl = Get-Acl -LiteralPath $path
        $ownerSid = $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
        if ($ownerSid -notin $allowedOwnerSids) {
            throw "Registration refused: $path is not owned by Administrators, SYSTEM, or TrustedInstaller."
        }
        foreach ($rule in $acl.Access) {
            if ($rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) { continue }
            $ruleRights = [int]$rule.FileSystemRights
            try {
                $ruleSid = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
            } catch {
                if (($ruleRights -band $writeMask) -ne 0) {
                    throw "Registration refused: Cannot resolve ACL identity with write rights on ${path}: $($rule.IdentityReference)."
                }
                continue
            }
            if (($ruleSid -notin $trustedWriterSids) -and ($ruleSid -ne $creatorOwnerSid) -and (($ruleRights -band $writeMask) -ne 0)) {
                throw "Registration refused: $path grants write rights to non-trusted SID $($rule.IdentityReference) ($($rule.FileSystemRights))."
            }
        }
    }

    return $exe
}

function New-ConnectedTaskScheduler {
    $service = New-Object -ComObject "Schedule.Service"
    $service.Connect()
    return $service
}

function Get-TaskIfPresent {
    param($Folder, [string]$Name)
    try {
        return $Folder.GetTask($Name)
    } catch {
        if ($_.Exception.HResult -in @(-2147216625, -2147024894)) { return $null }
        throw
    }
}

function Assert-ManagedTask {
    param($Task)

    $definition = $Task.Definition
    if ($definition.RegistrationInfo.Description -ne $TaskDescription) {
        throw "Operation refused: A task with this name was not created by the AeroTune test-task setup."
    }
    if ($definition.Actions.Count -ne 1) {
        throw "Operation refused: The task does not have exactly one expected action."
    }
    $taskExe = [System.IO.Path]::GetFullPath($definition.Actions.Item(1).Path)
    $programFiles = [System.IO.Path]::GetFullPath([System.Environment]::GetFolderPath("ProgramFiles"))
    $prefix = $programFiles.TrimEnd([char]92) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $taskExe.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Operation refused: The task target is not under Program Files: $taskExe"
    }
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
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
        throw "Operation refused: The task principal could not be resolved to a user SID."
    }
    if ($taskUserSid -ne $identity.User.Value) {
        throw "Operation refused: The task principal does not match the current user SID."
    }
    if (($definition.Principal.LogonType -ne 3) -or ($definition.Principal.RunLevel -ne 1)) {
        throw "Operation refused: The task is not configured for this interactive user at highest run level."
    }
    if ($definition.Triggers.Count -ne 0) {
        throw "Operation refused: The task has an automatic trigger; this test task must remain on-demand only."
    }
    if ($definition.Actions.Item(1).Arguments) {
        throw "Operation refused: The test task must not carry command-line arguments."
    }
    return $taskExe
}

function Assert-TaskMatchesTarget {
    param($Task, [string]$VerifiedExe)
    $taskExe = Assert-ManagedTask -Task $Task
    if (-not $taskExe.Equals($VerifiedExe, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Operation refused: The task target has changed: $taskExe"
    }
}

switch ($Action) {
    "Check" {
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $token = if (Test-AdministratorToken) { "administrator token" } else { "standard token" }
        Write-Output "Protected path check passed: $verified"
        Write-Output "Current PowerShell token: $token. Check does not create or start a task."
    }
    "Register" {
        Assert-AdministratorToken
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
        $userSid = $identity.User.Value
        $taskSddl = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;$userSid)"
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $existing = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if ($existing) {
            Assert-TaskMatchesTarget -Task $existing -VerifiedExe $verified
        }

        $definition = $service.NewTask(0)
        $definition.RegistrationInfo.Description = $TaskDescription
        $definition.Principal.UserId = $identity.Name
        $definition.Principal.LogonType = 3  # TASK_LOGON_INTERACTIVE_TOKEN
        $definition.Principal.RunLevel = 1   # TASK_RUNLEVEL_HIGHEST
        $definition.Settings.AllowDemandStart = $true
        $definition.Settings.StartWhenAvailable = $false
        $definition.Settings.MultipleInstances = 2 # TASK_INSTANCES_IGNORE_NEW
        $definition.Settings.ExecutionTimeLimit = "PT0S"
        $definition.Settings.DisallowStartIfOnBatteries = $false
        $definition.Settings.StopIfGoingOnBatteries = $false
        $taskAction = $definition.Actions.Create(0) # TASK_ACTION_EXEC
        $taskAction.Path = $verified
        $taskAction.WorkingDirectory = Split-Path -Parent $verified
        $taskAction.Arguments = ""

        $folder.RegisterTaskDefinition($TaskName, $definition, 6, $identity.Name, $null, 3, $taskSddl) | Out-Null
        Write-Output "Registered manual-start task: $TaskName"
        Write-Output "No logon/startup trigger; standard users can read and run the task but cannot modify its action."
    }
    "Start" {
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $task = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if (-not $task) { throw "Task not found: $TaskName. Run Register first." }
        Assert-TaskMatchesTarget -Task $task -VerifiedExe $verified
        $processName = [System.IO.Path]::GetFileNameWithoutExtension($verified)
        if (Get-Process -Name $processName -ErrorAction SilentlyContinue) {
            throw "Close any existing AeroTune instance before starting the elevated test task."
        }
        $running = $task.Run($null)
        Write-Output "Requested an elevated AeroTune launch through Task Scheduler; instance ID: $($running.InstanceGuid)"
    }
    "Remove" {
        Assert-AdministratorToken
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $task = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if (-not $task) { Write-Output "Task not found: $TaskName"; break }
        [void](Assert-ManagedTask -Task $task)
        $folder.DeleteTask($TaskName, 0)
        Write-Output "Removed task: $TaskName"
    }
}
