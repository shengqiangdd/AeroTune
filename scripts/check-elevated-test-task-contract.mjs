import { readFileSync } from "node:fs";

const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const script = readFileSync("scripts/elevated-test-task.ps1", "utf8");
const helper = readFileSync("scripts/install-and-register-elevated-test-task.ps1", "utf8");
const checks = [
  [config.bundle?.windows?.nsis?.installMode === "perMachine", "NSIS installs into the protected per-machine location"],
  [/function Resolve-ProtectedExecutable/.test(script) && /GetFolderPath\("ProgramFiles"\)/.test(script) && /GetFileName\(\$exe\).*AeroTune\.exe/.test(script) && /Split-Path -Leaf \$installDirectory/.test(script), "task target must be AeroTune.exe in its protected Program Files folder"],
  [/trustedWriterSids/.test(script) && /Get-Acl -LiteralPath/.test(script) && /FileSystemRights/.test(script) && /0x40000000/.test(script) && /Stack\[string\]/.test(script) && !/FileSystemRights\]::(?:Modify|FullControl)/.test(script), "task setup verifies the full install tree and tests write bits without read-right false positives"],
  [/function Assert-AdministratorToken/.test(script) && /Register"\s*\{\s*Assert-AdministratorToken/.test(script), "task registration requires an already elevated token"],
  [!/Start-Process|Verb\s+RunAs/i.test(script), "setup never launches or bypasses UAC"],
  [/Principal\.LogonType = 3/.test(script) && /Principal\.RunLevel = 1/.test(script) && /NTAccount/.test(script) && /taskUserSid -ne \$identity\.User\.Value/.test(script), "task resolves the stored principal to the current SID at highest run level"],
  [/A;;FRFX;;;\$userSid/.test(script) && /A;;FA;;;BA/.test(script) && /A;;FA;;;SY/.test(script), "task DACL allows user execution but reserves changes for admins/system"],
  [/StartWhenAvailable = \$false/.test(script) && /Triggers\.Count -ne 0/.test(script) && !/Triggers\.Create/.test(script), "task has no automatic logon/startup trigger"],
  [/Actions\.Create\(0\)/.test(script) && /taskAction\.Arguments = ""/.test(script) && /Get-Process -Name \$processName/.test(script) && /task\.Run\(\$null\)/.test(script), "task starts only the fixed AeroTune executable without arguments and avoids ignored duplicate launches"],
  [/DeleteTask\(\$TaskName, 0\)/.test(script), "task has an explicit administrator-only removal path"],
  [/ExpectedInstallerHash/.test(helper) && /Get-FileHash/.test(helper) && /E4CD164809B9FE948E0F714682A9D6FAA0C48A6A5A526E558111C451F0A4CF28/.test(helper), "installer helper pins and verifies the built release hash"],
  [/IsInRole\([\s\S]*Administrator/.test(helper) && /Close AeroTune normally/.test(helper) && !/Stop-Process|taskkill/i.test(helper), "installer helper requires elevation and never kills AeroTune"],
  [/Start-Process -FilePath \$InstallerPath -Wait/.test(helper) && /-Action Check -ExecutablePath \$target/.test(helper) && /-Action Register -ExecutablePath \$target/.test(helper), "installer helper validates Program Files before registering"],
  [/Triggers\.Count -ne 0/.test(helper) && /GetSecurityDescriptor\(4\)/.test(helper) && /RawSecurityDescriptor/.test(helper) && /ReadAndExecute/.test(helper) && /writeMask/.test(helper) && /taskUserSid -ne \$identity\.User\.Value/.test(helper), "installer helper decodes normalized DACL rights and verifies user identity"],
];

let failures = 0;
for (const [passed, description] of checks) {
  if (passed) console.log(`PASS ${description}`);
  else {
    failures += 1;
    console.error(`FAIL ${description}`);
  }
}
if (failures) process.exit(1);
console.log(`Elevated test-task contract passed: ${checks.length} checks`);
