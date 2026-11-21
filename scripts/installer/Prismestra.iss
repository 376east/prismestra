#include "generated.iss"

[Setup]
AppId={{{#InstallerAppId}}
AppName={#ProductName}
AppVersion={#InstallerVersion}
VersionInfoVersion={#InstallerVersion}
AppPublisher=Prismestra
AppPublisherURL=https://github.com/376east/prismestra
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
Name: "pawnio"; Description: "Enable supported RAM and motherboard lighting (shared PawnIO driver)"
#endif

[Files]
#ifndef LifecycleTest
Source: "prerequisites\Prismestra-Setup-Probe.exe"; Flags: dontcopy solidbreak
#endif
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

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ProductKey = 'Software\Prismestra\Installer\{#InstallerAppId}';
  VcKey = 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64';
  ProtectedModules = '$env:PSModulePath = $PSHOME + ''\Modules''; ';

var
  RebootRequired: Boolean;
  VcRuntimeRestartRequired: Boolean;
#ifndef LifecycleTest
  WebViewProbeExtracted: Boolean;
#endif

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

#include "guard-native.iss"

function RunOriginalInstalledCommand(const Arguments: String): Boolean;
var
  Code: Integer;
begin
  try
    Result := GuardInstall('Check') and
      ExecAsOriginalUser(InstallPath + '\Prismestra.exe', Arguments,
        InstallPath, SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
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

function SetupProbePath: String;
begin
  if not WebViewProbeExtracted then begin
    ExtractTemporaryFile('Prismestra-Setup-Probe.exe');
    WebViewProbeExtracted := True;
  end;
  Result := ExpandConstant('{tmp}\Prismestra-Setup-Probe.exe');
  if CompareText(GetSHA256OfFile(Result), '{#WebViewProbeSha256}') <> 0 then
    RaiseException('Native Setup probe failed its SHA-256 check.');
end;

function RunOriginalSetupProbe(const Arguments: String; var Code: Integer): Boolean;
var
  Source, Suffix, Directory, Image: String;
  Bytes: AnsiString;
  Attempt, I: Integer;
begin
  Result := False;
  try
    Source := SetupProbePath;
    { The elevated user's private temporary directory cannot be assumed readable
      when the account used for UAC differs from Setup's original user. }
    Suffix := Lowercase(ExtractFileName(ExpandConstant('{tmp}')));
    if (Length(Suffix) < 8) or (Length(Suffix) > 40) or
      (Copy(Suffix, 1, 3) <> 'is-') or (Copy(Suffix, Length(Suffix) - 3, 4) <> '.tmp') then Exit;
    for I := 4 to Length(Suffix) - 4 do
      if not (((Suffix[I] >= 'a') and (Suffix[I] <= 'z')) or ((Suffix[I] >= '0') and (Suffix[I] <= '9')))
        then Exit;
    if not GuardInstall('Check') then Exit;
    Directory := ExpandConstant('{autopf}\Prismestra Setup Probe-') + Suffix;
    Image := Directory + '\Prismestra-Setup-Probe.exe';
    GuardPrepareDirectory(Directory, False);
    try
      if not LoadStringFromFile(Source, Bytes) or not SaveStringToFile(Image, Bytes, False) then Exit;
      if CompareText(GetSHA256OfFile(Image), '{#WebViewProbeSha256}') <> 0 then Exit;
      Result := ExecAsOriginalUser(Image, Arguments, ExpandConstant('{sys}'), SW_HIDE,
        ewWaitUntilTerminated, Code);
    finally
      for Attempt := 1 to 5 do begin
        if not FileExists(Image) or DeleteFile(Image) then Break;
        Sleep(100);
      end;
      if not RemoveDir(Directory) then Log('Could not remove the protected Setup probe directory.');
    end;
  except
    Log(GetExceptionMessage);
    Result := False;
  end;
end;

function OriginalUserWebViewReady: Boolean;
var
  Code: Integer;
begin
  Result := RunOriginalSetupProbe('', Code) and (Code = 0);
end;

function WebViewReady: Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(HKLM32,
    'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'pv', Version) and SupportedWebViewVersion(Version);
  if not Result then Result := OriginalUserWebViewReady;
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
  if VcRuntimeRestartRequired then begin
    try
      if not ExecAndCaptureOutput(SetupProbePath, '--current-sid',
        ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code, Output) then Exit;
    except
      Log(GetExceptionMessage);
      Exit;
    end;
  end else begin
    if not GuardInstall('Check') or not ExecAndCaptureOutput(InstallPath + '\Prismestra.exe',
      '--setup-current-user', ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code, Output) then Exit;
  end;
  if (Code = 0) and not Output.Error and (GetArrayLength(Output.StdOut) = 1) and
    ValidSID(Trim(Output.StdOut[0])) then Result := Trim(Output.StdOut[0]);
end;

function ProbeOriginalUser(const SID: String): Boolean;
var
  Code: Integer;
begin
  Result := False;
  if not ValidSID(SID) then Exit;
  if VcRuntimeRestartRequired then
    Result := RunOriginalSetupProbe('--check-user ' + SID, Code) and (Code = 0)
  else
    Result := RunOriginalInstalledCommand('--setup-check-user ' + SID);
end;

function RunManagedPolicy(const Operation, SID: String): Boolean;
var
  Tool: String;
  Code: Integer;
begin
  Tool := InstallPath + '\Prismestra-Setup-Policy.exe';
  Result := ValidSID(SID) and FileExists(Tool) and
    (GetSHA256OfFile(Tool) = '{#PolicyToolSha256}') and
    ExecAndLogOutput(Tool, Operation + ' ' + SID, ExpandConstant('{sys}'),
      SW_HIDE, ewWaitUntilTerminated, Code, nil) and (Code = 0);
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
  if not ProbeOriginalUser(SID) then begin
    Log('Managed PC task needs matching ordinary and elevated identities; ordinary backend available.');
    Exit;
  end;
  { Record the protected recovery intent before provisioning an owned task. }
  if not RegWriteStringValue(HKLM64, ProductKey, 'PolicySid', SID) then begin
    Log('Could not record managed PC policy ownership; no task provisioned.');
    Exit;
  end;
  if not (GuardInstall('Check') and RunManagedPolicy('--install', SID)) then
    Log('Managed PC task unavailable; retained ownership intent for safe cleanup.');
end;

function LegacyStartup(const Action, SID: String): Boolean;
var
  Code: Integer;
begin
  Result := GuardInstall('Check') and ExecAndLogOutput(PowerShell,
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + ProtectedModules +
    '& ' + PSQuote(InstallPath + '\retire-legacy-startup.ps1') +
    ' -OriginalSid ' + PSQuote(SID) + ' -Action ' + PSQuote(Action) + '; exit $LASTEXITCODE"',
    ExpandConstant('{sys}'), SW_HIDE, ewWaitUntilTerminated, Code, nil) and (Code = 0);
end;

procedure RetireLegacyStartup;
var
  SID: String;
begin
  SID := CurrentSID;
  if not ProbeOriginalUser(SID) then begin
    Log('Legacy startup migration requires matching original and elevated user identities.');
    Exit;
  end;
  if not LegacyStartup('Prepare', SID) then begin
    Log('Legacy startup source could not be prepared; previous startup entries kept.');
    Exit;
  end;
  if RebootRequired then begin
    Log('Verified legacy source saved for next launch; profile preparation and prior task retirement require Setup after restart.');
    Exit;
  end;
  if not RunOriginalInstalledCommand('--setup-prepare-installed ' + SID) then begin
    Log('Legacy profile could not be prepared as the original ordinary user; previous startup entries kept.');
    Exit;
  end;
  if not LegacyStartup('Disable', SID) then
    Log('Legacy startup migration was deferred; previous startup definitions were preserved.');
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
    else if (Code = 3010) or (Code = 1641) then begin
      VcRuntimeRestartRequired := True;
      RebootRequired := True;
    end
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
var
  StartupReady: Boolean;
#ifndef LifecycleTest
  ProbeCode: Integer;
#endif
begin
  if Step = ssPostInstall then begin
    if not GuardInstall('Check') then
      RaiseException('The installed Prismestra files did not pass access checks.');
#ifndef LifecycleTest
    InstallPolicy;
#endif
    StartupReady := False;
    if WizardIsTaskSelected('autostart') then begin
      { The app imports the VC++ runtime, which may be unavailable until restart. }
#ifndef LifecycleTest
      if VcRuntimeRestartRequired then
        StartupReady := RunOriginalSetupProbe('--enable-startup "' + InstallPath + '\Prismestra.exe"', ProbeCode) and
          (ProbeCode = 0)
      else
#endif
        StartupReady := RunOriginalInstalledCommand('--setup-enable-startup');
      if not StartupReady then
        Log('Could not enable sign-in startup for the original ordinary user.');
    end else begin
#ifndef LifecycleTest
      if VcRuntimeRestartRequired then
        StartupReady := RunOriginalSetupProbe('--disable-startup "' + InstallPath + '\Prismestra.exe"', ProbeCode) and
          (ProbeCode = 0)
      else
#endif
        StartupReady := RunOriginalInstalledCommand('--setup-disable-startup');
      if not StartupReady then
        Log('Could not check sign-in startup for the original ordinary user.');
    end;
#ifndef LifecycleTest
    if StartupReady then RetireLegacyStartup
    else Log('Previous startup kept because the new startup preference could not be applied.');
#endif
  end;
  if (Step = ssDone) and not WizardSilent and not RebootRequired and
    WizardIsTaskSelected('launch') and not RunOriginalInstalledCommand('--setup-launch-installed') then
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
    if not RunManagedPolicy('--uninstall', SID) then
      RaiseException('Could not remove the installer-owned managed PC policy.');
    RegDeleteValue(HKLM64, ProductKey, 'PolicySid');
    RegDeleteKeyIfEmpty(HKLM64, ProductKey);
  end;
#endif
  RemoveOwnedStartup;
end;
