#include "generated.iss"

[Setup]
AppId={{{#InstallerAppId}}
AppName={#ProductName}
AppVersion={#InstallerVersion}
VersionInfoVersion={#InstallerVersion}
AppPublisher=Prismestra
AppPublisherURL=https://github.com/malyarq/prismestra
DefaultDirName={autopf}\{#ProductDirectory}
DisableDirPage=yes
UsePreviousAppDir=no
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.18362
CloseApplications=no
RestartApplications=no
RedirectionGuard=yes
UninstallDisplayName={#ProductName}
UninstallDisplayIcon={app}\Prismestra.exe
OutputDir=dist
OutputBaseFilename={#InstallerBaseName}
Compression=lzma2/max
SolidCompression=yes
DiskSpanning=no
WizardStyle=modern
SetupLogging=yes

[Tasks]
Name: "autostart"; Description: "Start Prismestra when I sign in"
Name: "launch"; Description: "Open Prismestra after installation"
#ifdef IncludePawnIO
Name: "pawnio"; Description: "Install the shared PawnIO driver for supported PC lighting"; Flags: unchecked
#endif

[Files]
Source: "payload\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs
#ifndef LifecycleTest
Source: "prerequisites\vc_redist.x64.exe"; Flags: dontcopy solidbreak
Source: "prerequisites\MicrosoftEdgeWebView2RuntimeInstallerX64.exe"; Flags: dontcopy solidbreak
#ifdef IncludePawnIO
Source: "prerequisites\PawnIO_setup.exe"; Flags: dontcopy solidbreak
#endif
#endif

[Icons]
Name: "{commonprograms}\{#ShortcutName}"; Filename: "{app}\Prismestra.exe"; Parameters: "--installed"; WorkingDir: "{app}"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ProductKey = 'Software\Prismestra\Installer\{#InstallerAppId}';
  VcKey = 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64';
  ProtectedModules = '$env:PSModulePath = $PSHOME + ''\Modules''; ';

var
  RebootRequired: Boolean;

function PowerShell: String;
begin
  Result := ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe');
end;

function InstallPath: String;
begin
  Result := ExpandConstant('{app}');
end;

function FixedInstallPath: Boolean;
begin
  Result := CompareText(InstallPath, ExpandConstant('{autopf}\{#ProductDirectory}')) = 0;
end;

function PSQuote(Value: String): String;
begin
  StringChangeEx(Value, '''', '''''', True);
  Result := '''' + Value + '''';
end;

function ScriptArgs(const Encoded, Action, Extra: String): String;
begin
  Result := '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + ProtectedModules + '& ' +
    '([scriptblock]::Create([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(' +
    PSQuote(Encoded) + ')))) ' + PSQuote(Action) + ' ' + PSQuote(InstallPath) + Extra + '"';
end;

function GuardInstall(const Action: String): Boolean;
var
  Code: Integer;
begin
  Result := FixedInstallPath and ExecAndLogOutput(PowerShell,
    ScriptArgs('{#GuardCode}', Action, ''), ExpandConstant('{sys}'), SW_HIDE,
    ewWaitUntilTerminated, Code, nil) and (Code = 0);
end;

function OriginalUser(const Action, ExpectedSid: String): Boolean;
var
  Code: Integer;
begin
  try
    Result := ExecAsOriginalUser(PowerShell,
      ScriptArgs('{#UserCode}', Action, ' ' + PSQuote('{#StartupValue}') + ' ' + PSQuote(ExpectedSid)),
      ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
  except
    Log(GetExceptionMessage);
    Result := False;
  end;
end;

#ifndef LifecycleTest
function VcRuntimeReady: Boolean;
var
  Installed, Major, Minor, Build, Revision: Cardinal;
begin
  Result := False;
  if not RegQueryDWordValue(HKLM64, VcKey, 'Installed', Installed) or (Installed <> 1) or
    not RegQueryDWordValue(HKLM64, VcKey, 'Major', Major) or
    not RegQueryDWordValue(HKLM64, VcKey, 'Minor', Minor) or
    not RegQueryDWordValue(HKLM64, VcKey, 'Bld', Build) or
    not RegQueryDWordValue(HKLM64, VcKey, 'Rbld', Revision) then Exit;
  if Major <> {#VcMajor} then Result := Major > {#VcMajor}
  else if Minor <> {#VcMinor} then Result := Minor > {#VcMinor}
  else if Build <> {#VcBuild} then Result := Build > {#VcBuild}
  else Result := Revision >= {#VcRevision};
end;

function SupportedWebViewVersion(const Value: String): Boolean;
var
  VersionNumber: Int64;
begin
  Result := False;
  if not StrToVersion(Value, VersionNumber) then Exit;
  { StrToVersion accepts partial and extra components. Require the exact
    four-component format used by EdgeUpdate. }
  Result := (Value = VersionToStr(VersionNumber)) and
    (ComparePackedVersion(VersionNumber, PackVersionComponents(105, 0, 0, 0)) >= 0);
end;

function WebViewReady: Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(HKLM32,
    'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'pv', Version) and SupportedWebViewVersion(Version);
  if not Result then Result := OriginalUser('WebView', '');
end;

function PawnIOPresent: Boolean;
begin
  Result := RegKeyExists(HKLM64, 'SYSTEM\CurrentControlSet\Services\PawnIO');
end;

function InstallPrerequisite(const FileName, Parameters: String; var Code: Integer): Boolean;
begin
  ExtractTemporaryFile(FileName);
  Result := Exec(ExpandConstant('{tmp}\') + FileName, Parameters, ExpandConstant('{tmp}'),
    SW_HIDE, ewWaitUntilTerminated, Code);
  if Result then Log(FileName + ' exit code ' + IntToStr(Code))
  else Log(FileName + ' could not start: ' + IntToStr(Code));
end;

function ValidSID(const Value: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  if (Length(Value) < 8) or (Length(Value) > 184) or (Copy(Value, 1, 4) <> 'S-1-') then Exit;
  for I := 5 to Length(Value) do
    if not (((Value[I] >= '0') and (Value[I] <= '9')) or (Value[I] = '-')) then Exit;
  Result := True;
end;

function CurrentSID: String;
var
  Code: Integer;
  Output: TExecOutput;
begin
  Result := '';
  if ExecAndCaptureOutput(PowerShell,
    '-NoProfile -NonInteractive -Command "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value"',
    ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code, Output) and
    (Code = 0) and not Output.Error and (GetArrayLength(Output.StdOut) = 1) and
    ValidSID(Trim(Output.StdOut[0])) then Result := Trim(Output.StdOut[0]);
end;

function Policy(const Action, SID: String): Boolean;
var
  Code: Integer;
begin
  Result := ValidSID(SID) and GuardInstall('Check') and ExecAndLogOutput(PowerShell,
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + ProtectedModules +
    '& ' + PSQuote(InstallPath + '\installer_policy.ps1') + ' -Action ' + PSQuote(Action) +
    ' -InstallRoot ' + PSQuote(InstallPath) + ' -UserSid ' + PSQuote(SID) + '; exit $LASTEXITCODE"',
    ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code, nil) and (Code = 0);
end;

procedure InstallPolicy;
var
  SID, PreviousSID: String;
begin
  SID := CurrentSID;
  if RegQueryStringValue(HKLM64, ProductKey, 'PolicySid', PreviousSID) and
    (PreviousSID <> SID) then begin
    Log('Existing managed PC policy belongs to a different administrator; no new policy provisioned.');
    Exit;
  end;
  if not PawnIOPresent then Exit;
  if not ValidSID(SID) or not OriginalUser('Probe', SID) then begin
    Log('Managed PC task needs matching ordinary and elevated identities; ordinary backend available.');
    Exit;
  end;
  { Record the protected recovery intent before provisioning an owned task. }
  if not RegWriteStringValue(HKLM64, ProductKey, 'PolicySid', SID) then begin
    Log('Could not record managed PC policy ownership; no task provisioned.');
    Exit;
  end;
  if not Policy('Install', SID) then
    Log('Managed PC task unavailable; retained ownership intent for safe cleanup.');
end;
#endif

function PrepareToInstall(var NeedsRestart: Boolean): String;
#ifndef LifecycleTest
var
  Code: Integer;
#endif
begin
  Result := '';
  if not GuardInstall('Stop') then begin
    Result := 'Prismestra could not close or its installation path could not be verified. ' +
      'Finish pending edits and exit Prismestra from the notification area, then retry.';
    Exit;
  end;
#ifndef LifecycleTest
  if not VcRuntimeReady then begin
    if not InstallPrerequisite('vc_redist.x64.exe', '/install /quiet /norestart', Code) then
      Result := 'Could not start the offline Visual C++ installer.'
    else if (Code = 3010) or (Code = 1641) then RebootRequired := True
    else if (Code <> 0) and (Code <> 1638) then
      Result := 'The required x64 Visual C++ runtime could not be installed.'
    else if not VcRuntimeReady then
      Result := 'Setup could not verify the minimum x64 Visual C++ runtime version.';
    if Result <> '' then Exit;
  end;
  if not WebViewReady then begin
    if not InstallPrerequisite('MicrosoftEdgeWebView2RuntimeInstallerX64.exe', '/silent /install', Code) then
      Result := 'Could not start the offline WebView2 installer.'
    else if (Code = 3010) or (Code = 1641) then RebootRequired := True
    else if (Code <> 0) and (Code <> 1638) then
      Result := 'The required WebView2 Runtime could not be installed.'
    else if not WebViewReady then
      Result := 'Setup could not verify the required WebView2 Runtime.';
    if Result <> '' then Exit;
  end;
#ifdef IncludePawnIO
  if WizardIsTaskSelected('pawnio') and not PawnIOPresent then begin
    if not InstallPrerequisite('PawnIO_setup.exe', '-install -silent', Code) then
      Result := 'Could not start the selected PawnIO installer.'
    else if (Code = 3010) or (Code = 1641) then RebootRequired := True
    else if ((Code <> 0) and (Code <> 183)) or not PawnIOPresent then
      Result := 'The selected PawnIO driver could not be installed.';
  end;
#endif
#endif
  if (Result = '') and not GuardInstall('Prepare') then
    Result := 'Setup could not prepare the protected Program Files directory.';
end;

function NeedRestart: Boolean;
begin
  Result := RebootRequired;
end;

procedure CurStepChanged(Step: TSetupStep);
begin
  if Step = ssPostInstall then begin
    if not GuardInstall('Check') then
      RaiseException('The installed Prismestra files did not pass access checks.');
#ifndef LifecycleTest
    InstallPolicy;
#endif
    if WizardIsTaskSelected('autostart') then begin
      if not OriginalUser('EnableStartup', '') then
        Log('Could not enable sign-in startup for the original ordinary user.');
    end else if not OriginalUser('DisableStartup', '') then
      Log('Could not check sign-in startup for the original ordinary user.');
  end;
  if (Step = ssDone) and not WizardSilent and not RebootRequired and
    WizardIsTaskSelected('launch') and not OriginalUser('Launch', '') then
    Log('Prismestra start deferred. Open it from the Start menu under your ordinary account.');
end;

procedure RemoveOwnedStartup;
var
  Hives: TArrayOfString;
  I: Integer;
  Key, Command: String;
begin
  if not RegGetSubkeyNames(HKU, '', Hives) then Exit;
  for I := 0 to GetArrayLength(Hives) - 1 do begin
    Key := Hives[I] + '\' + RunKey;
    if RegQueryStringValue(HKU, Key, '{#StartupValue}', Command) and
      (Command = '"' + InstallPath + '\Prismestra.exe" --installed --autostart') then
      RegDeleteValue(HKU, Key, '{#StartupValue}');
  end;
end;

procedure CurUninstallStepChanged(Step: TUninstallStep);
#ifndef LifecycleTest
var
  SID: String;
#endif
begin
  if Step <> usUninstall then Exit;
  if not GuardInstall('Stop') then
    RaiseException('Prismestra is still running or its installation path could not be verified. ' +
      'Finish pending edits and exit it from the notification area before uninstalling.');
#ifndef LifecycleTest
  if RegQueryStringValue(HKLM64, ProductKey, 'PolicySid', SID) then begin
    if not Policy('Uninstall', SID) then
      RaiseException('Could not remove the installer-owned managed PC policy.');
    RegDeleteValue(HKLM64, ProductKey, 'PolicySid');
    RegDeleteKeyIfEmpty(HKLM64, ProductKey);
  end;
#endif
  RemoveOwnedStartup;
end;
