[CmdletBinding()]
param(
    [ValidateSet("Check", "Verify", "Run", "Register", "Remove")]
    [string]$Action = "Check",
    [string]$ExecutablePath = (Join-Path ([System.Environment]::GetFolderPath("ProgramFiles")) "AeroTune\AeroTune.exe")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$TaskName = "AeroTune-SignedUpdater"
$TaskDescription = "AeroTune signed update runner; fixed protected executable and no automatic trigger."
$UpdateArgument = "--aerotune-update-only"
$RegistrationEnabled = $false # Flip only after signed rollback and privileged Windows review pass.

function Test-AdministratorToken {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-AdministratorToken {
    if (-not (Test-AdministratorToken)) {
        throw "Register/Remove require an already elevated PowerShell; this script never launches or bypasses UAC."
    }
}

function Resolve-ProtectedExecutable {
    param([Parameter(Mandatory = $true)][string]$Candidate)

    if (-not (Test-Path -LiteralPath $Candidate -PathType Leaf)) {
        throw "AeroTune executable not found: $Candidate"
    }
    $exe = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Candidate).Path)
    $programFiles = [System.IO.Path]::GetFullPath([System.Environment]::GetFolderPath("ProgramFiles"))
    $prefix = $programFiles.TrimEnd([char]92) + [System.IO.Path]::DirectorySeparatorChar
    $expectedExe = [System.IO.Path]::GetFullPath((Join-Path $programFiles "AeroTune\AeroTune.exe"))
    if (-not $exe.Equals($expectedExe, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Registration refused: target must be exactly the protected Program Files/AeroTune/AeroTune.exe path: $expectedExe"
    }
    $installDirectory = Split-Path -Parent $exe

    $trustedInstallerSid = ""
    try {
        $trustedInstallerSid = ([System.Security.Principal.NTAccount]::new("NT SERVICE", "TrustedInstaller")).Translate([System.Security.Principal.SecurityIdentifier]).Value
    } catch { }
    $allowedOwnerSids = @("S-1-5-18", "S-1-5-32-544")
    $trustedWriterSids = @("S-1-5-18", "S-1-5-32-544")
    if ($trustedInstallerSid) { $allowedOwnerSids += $trustedInstallerSid; $trustedWriterSids += $trustedInstallerSid }
    $writeMask = [int](
        [System.Security.AccessControl.FileSystemRights]::Write -bor
        [System.Security.AccessControl.FileSystemRights]::Delete -bor
        [System.Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor
        [System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
        [System.Security.AccessControl.FileSystemRights]::TakeOwnership -bor
        [int]0x10000000 -bor
        [int]0x40000000
    )

    $pathsToCheck = [System.Collections.Generic.List[string]]::new()
    $pathsToCheck.Add($exe)
    $cursor = Split-Path -Parent $exe
    while ($true) {
        $pathsToCheck.Add($cursor)
        if ($cursor.TrimEnd([char]92).Equals($programFiles.TrimEnd([char]92), [System.StringComparison]::OrdinalIgnoreCase)) { break }
        if (-not $cursor.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Registration refused: unexpected path component between Program Files and target: $cursor"
        }
        $parent = Split-Path -Parent $cursor
        if (-not $parent -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Unable to validate install-path ACL: $cursor"
        }
        $cursor = $parent
    }
    $pendingDirectories = [System.Collections.Generic.Stack[string]]::new()
    $pendingDirectories.Push($installDirectory)
    while ($pendingDirectories.Count -gt 0) {
        $directory = $pendingDirectories.Pop()
        foreach ($entry in Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop) {
            $pathsToCheck.Add($entry.FullName)
            if ($entry.PSIsContainer) {
                if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "Registration refused: install tree contains a reparse point or symlink: $($entry.FullName)"
                }
                $pendingDirectories.Push($entry.FullName)
            }
        }
    }
    foreach ($path in $pathsToCheck) {
        $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Registration refused: target path contains a reparse point or symlink: $path"
        }
        $acl = Get-Acl -LiteralPath $path
        $ownerSid = $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
        if ($ownerSid -notin $allowedOwnerSids) { throw "Registration refused: $path is not owned by Administrators, SYSTEM, or TrustedInstaller." }
        foreach ($rule in $acl.Access) {
            if ($rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow) { continue }
            $ruleRights = [int]$rule.FileSystemRights
            try { $ruleSid = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value }
            catch {
                if (($ruleRights -band $writeMask) -ne 0) { throw "Registration refused: cannot resolve a write-capable ACL identity on $path." }
                continue
            }
            if (($ruleSid -notin $trustedWriterSids) -and ($ruleSid -ne "S-1-3-0") -and (($ruleRights -band $writeMask) -ne 0)) {
                throw "Registration refused: $path grants write rights to a non-trusted SID."
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
    try { return $Folder.GetTask($Name) }
    catch {
        if ($_.Exception.HResult -in @(-2147216625, -2147024894)) { return $null }
        throw
    }
}

function Assert-ManagedTask {
    param($Task, [string]$VerifiedExe)

    if ($Task.Path -ne "\$TaskName") { throw "Operation refused: registered task path differs from the fixed task name." }
    if (-not $Task.Enabled) { throw "Operation refused: updater task is disabled." }
    $definition = $Task.Definition
    if ($definition.RegistrationInfo.Description -ne $TaskDescription) {
        throw "Operation refused: existing task was not created by AeroTune signed-updater setup."
    }
    if ($definition.Actions.Count -ne 1) { throw "Operation refused: expected exactly one fixed task action." }
    $action = $definition.Actions.Item(1)
    if ($action.Type -ne 0) { throw "Operation refused: updater task action must be an Exec action." }
    if (-not ([System.IO.Path]::GetFullPath($action.Path)).Equals($VerifiedExe, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Operation refused: task executable path differs from the protected AeroTune binary."
    }
    if ($action.Arguments -ne $UpdateArgument) { throw "Operation refused: task arguments differ from the fixed update-only mode." }
    if (-not ([System.IO.Path]::GetFullPath($action.WorkingDirectory)).Equals((Split-Path -Parent $VerifiedExe), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Operation refused: task working directory differs from the protected install directory."
    }
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $taskUserId = [string]$definition.Principal.UserId
    try {
        if ($taskUserId -match "^S-\d-") {
            $taskUserSid = [System.Security.Principal.SecurityIdentifier]::new($taskUserId).Value
        } else {
            $taskUserSid = ([System.Security.Principal.NTAccount]::new($taskUserId)).Translate([System.Security.Principal.SecurityIdentifier]).Value
        }
    } catch { throw "Operation refused: task principal could not be resolved to a SID." }
    if ($taskUserSid -ne $identity.User.Value) { throw "Operation refused: task principal differs from the registering user." }
    if (($definition.Principal.LogonType -ne 3) -or ($definition.Principal.RunLevel -ne 1)) {
        throw "Operation refused: task must use this interactive user at highest run level."
    }
    if ($definition.Triggers.Count -ne 0) { throw "Operation refused: updater task must have no automatic triggers." }
    if (-not $definition.Settings.AllowDemandStart) { throw "Operation refused: on-demand start is disabled." }
    if ($definition.Settings.StartWhenAvailable -or ($definition.Settings.MultipleInstances -ne 2)) {
        throw "Operation refused: updater task must be demand-only and ignore duplicate starts."
    }
    if ($definition.Settings.DisallowStartIfOnBatteries -or $definition.Settings.StopIfGoingOnBatteries) {
        throw "Operation refused: updater task must not be suspended or blocked on battery power."
    }
    if ($definition.Settings.ExecutionTimeLimit -ne "PT0S") {
        throw "Operation refused: updater task must not be force-stopped during a protected install transaction."
    }

    $parsedDacl = [System.Security.AccessControl.RawSecurityDescriptor]::new($Task.GetSecurityDescriptor(7))
    $administratorsSid = "S-1-5-32-544"
    $systemSid = "S-1-5-18"
    if (($parsedDacl.Owner.Value -ne $administratorsSid) -or ($parsedDacl.Group.Value -ne $administratorsSid)) {
        throw "Operation refused: updater task owner/group must remain Administrators."
    }
    if (($parsedDacl.ControlFlags -band [System.Security.AccessControl.ControlFlags]::DiscretionaryAclProtected) -eq 0) {
        throw "Operation refused: updater task DACL must be protected from inheritance."
    }
    if ($parsedDacl.DiscretionaryAcl.Count -ne 3) {
        throw "Operation refused: task DACL contains unexpected principals."
    }
    $allowedTaskSids = @($systemSid, $administratorsSid, $identity.User.Value)
    $seenTaskSids = @{}
    foreach ($ace in $parsedDacl.DiscretionaryAcl) {
        if ($ace.AceType -ne [System.Security.AccessControl.AceType]::AccessAllowed) {
            throw "Operation refused: task DACL contains an object, callback, or non-allow ACE."
        }
        if ($ace.AceFlags -ne [System.Security.AccessControl.AceFlags]::None) {
            throw "Operation refused: task DACL contains inherited or propagation ACE flags."
        }
        if ($ace.AceQualifier -ne [System.Security.AccessControl.AceQualifier]::AccessAllowed) {
            throw "Operation refused: task DACL contains a non-allow ACE."
        }
        $sid = $ace.SecurityIdentifier.Value
        if ($sid -notin $allowedTaskSids) { throw "Operation refused: task DACL grants rights to an unexpected principal." }
        $seenTaskSids[$sid] = [int]$ace.AccessMask
        if (($sid -eq $systemSid) -or ($sid -eq $administratorsSid)) {
            $allAccess = [int]0x000F01FF
            if ((([int]$ace.AccessMask -band $allAccess) -ne $allAccess) -and (([int]$ace.AccessMask -band [int]0x10000000) -eq 0)) {
                throw "Operation refused: SYSTEM and Administrators must retain full task control."
            }
        }
    }
    foreach ($sid in $allowedTaskSids) {
        if (-not $seenTaskSids.ContainsKey($sid)) { throw "Operation refused: task DACL is missing a required principal." }
    }
    $userMask = 0
    foreach ($ace in $parsedDacl.DiscretionaryAcl) {
        if (($ace.SecurityIdentifier.Value -eq $identity.User.Value) -and
            ($ace.AceQualifier -eq [System.Security.AccessControl.AceQualifier]::AccessAllowed)) {
            $userMask = $userMask -bor [int]$ace.AccessMask
        }
    }
    $readExecuteMask = [int](
        [System.Security.AccessControl.FileSystemRights]::ReadAndExecute -bor
        [System.Security.AccessControl.FileSystemRights]::Synchronize
    )
    $writeMask = [int]0x10000000 -bor [int]0x40000000 -bor
        [int][System.Security.AccessControl.FileSystemRights]::Write -bor
        [int][System.Security.AccessControl.FileSystemRights]::Delete -bor
        [int][System.Security.AccessControl.FileSystemRights]::ChangePermissions -bor
        [int][System.Security.AccessControl.FileSystemRights]::TakeOwnership
    if (($userMask -band $readExecuteMask) -ne $readExecuteMask) { throw "Operation refused: user lacks task read/run rights." }
    if (($userMask -band $writeMask) -ne 0) { throw "Operation refused: user can modify the elevated task." }
    if (($userMask -band (-bnot $readExecuteMask)) -ne 0) { throw "Operation refused: user has unexpected task rights." }
    return $VerifiedExe
}

switch ($Action) {
    "Check" {
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $service = New-ConnectedTaskScheduler
        $task = Get-TaskIfPresent -Folder $service.GetFolder("\") -Name $TaskName
        if ($task) { [void](Assert-ManagedTask -Task $task -VerifiedExe $verified); Write-Output "Protected updater task is registered and matches policy." }
        else { Write-Output "Protected updater task is not registered; Check made no changes." }
    }
    "Verify" {
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $task = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if (-not $task) { throw "Operation refused: protected updater task is not registered." }
        [void](Assert-ManagedTask -Task $task -VerifiedExe $verified)
        Write-Output "Protected updater task definition and DACL match policy."
    }
    "Run" {
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $task = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if (-not $task) { throw "Operation refused: protected updater task is not registered." }
        [void](Assert-ManagedTask -Task $task -VerifiedExe $verified)
        [void]$task.Run($null)
        Write-Output "Verified updater task was queued through Task Scheduler COM."
    }
    "Register" {
        if (-not $RegistrationEnabled) { throw "Updater task registration is disabled until signed rollback and privileged Windows validation are complete." }
        Assert-AdministratorToken
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $existing = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if ($existing) {
            [void](Assert-ManagedTask -Task $existing -VerifiedExe $verified)
            Write-Output "Task already exists and matches policy; no changes made."
            break
        }

        $taskSddl = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;$($identity.User.Value))"
        $definition = $service.NewTask(0)
        $definition.RegistrationInfo.Description = $TaskDescription
        $definition.Principal.UserId = $identity.Name
        $definition.Principal.LogonType = 3
        $definition.Principal.RunLevel = 1
        $definition.Settings.AllowDemandStart = $true
        $definition.Settings.StartWhenAvailable = $false
        $definition.Settings.MultipleInstances = 2
        $definition.Settings.ExecutionTimeLimit = "PT0S"
        $definition.Settings.DisallowStartIfOnBatteries = $false
        $definition.Settings.StopIfGoingOnBatteries = $false
        $actionDefinition = $definition.Actions.Create(0)
        $actionDefinition.Path = $verified
        $actionDefinition.WorkingDirectory = Split-Path -Parent $verified
        $actionDefinition.Arguments = $UpdateArgument
        $folder.RegisterTaskDefinition($TaskName, $definition, 6, $identity.Name, $null, 3, $taskSddl) | Out-Null
        $registered = $folder.GetTask($TaskName)
        [void](Assert-ManagedTask -Task $registered -VerifiedExe $verified)
        Write-Output "Registered fixed, on-demand signed updater task; user can run it but cannot change its action."
    }
    "Remove" {
        Assert-AdministratorToken
        $verified = Resolve-ProtectedExecutable -Candidate $ExecutablePath
        $service = New-ConnectedTaskScheduler
        $folder = $service.GetFolder("\")
        $task = Get-TaskIfPresent -Folder $folder -Name $TaskName
        if (-not $task) { Write-Output "Task not found; no changes made."; break }
        [void](Assert-ManagedTask -Task $task -VerifiedExe $verified)
        $folder.DeleteTask($TaskName, 0)
        Write-Output "Removed managed signed updater task."
    }
}
