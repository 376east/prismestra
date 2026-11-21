{ The same guard runs before Setup and Uninstall use the installed executable.
  It imports only inbox Windows APIs. No code has to be loaded from the product
  directory to decide whether that directory is safe. }

const
  GuardMaximumEntries = 10000;
  GuardDirectoryAttribute = $10;
  GuardReparseAttribute = $400;
  GuardUnsafeWriteRights = $500D0156;
  GuardInheritOnlyAce = 8;
  GuardFileNotFound = 2;
  GuardPathNotFound = 3;
  GuardNoMoreFiles = 18;
  GuardAlreadyExists = 183;

type
  TGuardFileInfo = record
    Attributes: DWORD;
    CreatedLow, CreatedHigh, AccessedLow, AccessedHigh: DWORD;
    WrittenLow, WrittenHigh, Volume, SizeHigh, SizeLow: DWORD;
    Links, IndexHigh, IndexLow: DWORD;
  end;
  TGuardAclInfo = record
    Count, Used, Free: DWORD;
  end;
  TGuardAce = record
    Kind, Flags: Byte;
    Bytes: Word;
    Mask: DWORD;
  end;
  TGuardUsers = record
    Administrators, LocalSystem, TrustedInstaller: Cardinal;
  end;
  TGuardFindData = record
    Attributes: DWORD;
    CreatedLow, CreatedHigh, AccessedLow, AccessedHigh: DWORD;
    WrittenLow, WrittenHigh, SizeHigh, SizeLow, Reserved0, Reserved1: DWORD;
    FileName: array[0..259] of Char;
    AlternateName: array[0..13] of Char;
  end;
  TGuardSecurityAttributes = record
    Bytes: DWORD;
    Descriptor: Cardinal;
    Inherit: Integer;
  end;

function GuardOpen(Path: String; Access, Share: DWORD; Security: Cardinal;
  Creation, Flags: DWORD; Template: Cardinal): Cardinal;
  external 'CreateFileW@kernel32.dll stdcall';
function GuardFileInfo(FileHandle: Cardinal; var Info: TGuardFileInfo): Integer;
  external 'GetFileInformationByHandle@kernel32.dll stdcall';
function GuardSecurity(FileHandle: Cardinal; ObjectKind, Flags: DWORD; var Owner: Cardinal;
  Group: Cardinal; var Dacl: Cardinal; Sacl: Cardinal; var Descriptor: Cardinal): DWORD;
  external 'GetSecurityInfo@advapi32.dll stdcall';
function GuardAclInfo(Acl: Cardinal; var Info: TGuardAclInfo; Bytes, InfoClass: DWORD): Integer;
  external 'GetAclInformation@advapi32.dll stdcall';
function GuardAceAt(Acl: Cardinal; Index: DWORD; var Ace: Cardinal): Integer;
  external 'GetAce@advapi32.dll stdcall';
procedure GuardCopyAce(var Dest: TGuardAce; Source: Cardinal; Bytes: Cardinal);
  external 'RtlMoveMemory@ntdll.dll stdcall';
function GuardConvertSid(Value: String; var Sid: Cardinal): Integer;
  external 'ConvertStringSidToSidW@advapi32.dll stdcall';
function GuardEqualSid(One, Two: Cardinal): Integer;
  external 'EqualSid@advapi32.dll stdcall';
function GuardFreeLocal(Alloc: Cardinal): Cardinal;
  external 'LocalFree@kernel32.dll stdcall';
function GuardClose(FileHandle: Cardinal): Integer;
  external 'CloseHandle@kernel32.dll stdcall';
function GuardConvertSecurity(Value: String; Version: DWORD; var Descriptor: Cardinal;
  Bytes: Cardinal): Integer;
  external 'ConvertStringSecurityDescriptorToSecurityDescriptorW@advapi32.dll stdcall';
function GuardCreateDirectory(Path: String; var Security: TGuardSecurityAttributes): Integer;
  external 'CreateDirectoryW@kernel32.dll stdcall';
function GuardFindFirst(Pattern: String; var Data: TGuardFindData): Cardinal;
  external 'FindFirstFileW@kernel32.dll stdcall';
function GuardFindNext(Search: Cardinal; var Data: TGuardFindData): Integer;
  external 'FindNextFileW@kernel32.dll stdcall';
function GuardFindClose(Search: Cardinal): Integer;
  external 'FindClose@kernel32.dll stdcall';

procedure GuardFail(const Operation, Path: String; Code: Integer);
begin
  RaiseException(Format('%s: %s (%d)', [Operation, Path, Code]));
end;

function GuardTrustedSid(Sid: Cardinal; const Users: TGuardUsers): Boolean;
begin
  Result := (Sid <> 0) and ((GuardEqualSid(Sid, Users.Administrators) <> 0) or
    (GuardEqualSid(Sid, Users.LocalSystem) <> 0) or
    (GuardEqualSid(Sid, Users.TrustedInstaller) <> 0));
end;

procedure GuardCheckDacl(FileHandle: Cardinal; const Path: String;
  const Users: TGuardUsers; ProgramFilesAncestor: Boolean);
var
  Owner, Dacl, Descriptor, Address: Cardinal;
  Info: TGuardAclInfo;
  Ace: TGuardAce;
  Code, I: Integer;
begin
  Descriptor := 0;
  try
    Code := Integer(GuardSecurity(FileHandle, 1, 5, Owner, 0, Dacl, 0, Descriptor));
    if Code <> 0 then GuardFail('Cannot read security descriptor', Path, Code);
    if not GuardTrustedSid(Owner, Users) then GuardFail('Untrusted installation owner', Path, 0);
    if (Dacl = 0) or (GuardAclInfo(Dacl, Info, 12, 2) = 0) then
      GuardFail('Cannot read installation ACL', Path, DLLGetLastError);
    for I := 0 to Integer(Info.Count) - 1 do begin
      if (GuardAceAt(Dacl, DWORD(I), Address) = 0) or (Address = 0) then
        GuardFail('Cannot read installation ACL entry', Path, DLLGetLastError);
      GuardCopyAce(Ace, Address, 8);
      if not (ProgramFilesAncestor and ((Ace.Flags and GuardInheritOnlyAce) <> 0)) then begin
        if Ace.Kind = 0 then begin
          if Ace.Bytes < 16 then GuardFail('Invalid allow ACE', Path, 0);
          if ((Ace.Mask and GuardUnsafeWriteRights) <> 0) and not GuardTrustedSid(Address + 8, Users) then
            GuardFail('Installation is writable by an unprivileged identity', Path, 0);
        end else if Ace.Kind <> 1 then
          GuardFail('Unexpected installation ACL entry type', Path, Ace.Kind);
      end;
    end;
  finally
    if Descriptor <> 0 then GuardFreeLocal(Descriptor);
  end;
end;

function GuardReadEntry(const Path: String; const Users: TGuardUsers;
  ProgramFilesAncestor: Boolean; var Attributes: DWORD): Boolean;
var
  FileHandle: Cardinal;
  Info: TGuardFileInfo;
  Code: Integer;
begin
  Result := False;
  { READ_CONTROL | FILE_READ_ATTRIBUTES, OPEN_EXISTING,
    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT. }
  FileHandle := GuardOpen('\\?\' + Path, $20080, 7, 0, 3, $02200000, 0);
  if FileHandle = Cardinal(-1) then begin
    Code := DLLGetLastError;
    if (Code = GuardFileNotFound) or (Code = GuardPathNotFound) then Exit;
    GuardFail('Cannot open installation path', Path, Code);
  end;
  try
    if GuardFileInfo(FileHandle, Info) = 0 then
      GuardFail('Cannot read installation attributes', Path, DLLGetLastError);
    if (Info.Attributes and GuardReparseAttribute) <> 0 then
      GuardFail('Installation contains a reparse point', Path, 0);
    if ((Info.Attributes and GuardDirectoryAttribute) = 0) and (Info.Links <> 1) then
      GuardFail('Installation contains a hard-linked file', Path, 0);
    GuardCheckDacl(FileHandle, Path, Users, ProgramFilesAncestor);
    Attributes := Info.Attributes;
    Result := True;
  finally
    GuardClose(FileHandle);
  end;
end;

procedure GuardPrepareDirectory(const Path: String; AllowExisting: Boolean);
var
  Descriptor: Cardinal;
  Security: TGuardSecurityAttributes;
  Code: Integer;
begin
  Descriptor := 0;
  { Owner and writer ACEs are assigned atomically with creation. }
  if GuardConvertSecurity('O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FRFX;;;BU)',
    1, Descriptor, 0) = 0 then GuardFail('Cannot prepare installation security', Path, DLLGetLastError);
  try
    { SECURITY_ATTRIBUTES size for the pinned 32-bit Inno Setup engine. }
    Security.Bytes := 12;
    Security.Descriptor := Descriptor;
    Security.Inherit := 0;
    if GuardCreateDirectory('\\?\' + Path, Security) = 0 then begin
      Code := DLLGetLastError;
      if (Code <> GuardAlreadyExists) or not AllowExisting then GuardFail('Cannot create protected installation directory', Path, Code);
    end;
  finally
    if Descriptor <> 0 then GuardFreeLocal(Descriptor);
  end;
end;

function GuardFindName(const Data: TGuardFindData): String;
var
  I: Integer;
begin
  Result := '';
  for I := 0 to 259 do begin
    if Data.FileName[I] = #0 then Exit;
    Result := Result + Data.FileName[I];
  end;
  RaiseException('Unterminated installation file name.');
end;

procedure GuardCheckTree(const Root: String; const Users: TGuardUsers);
var
  Pending: TArrayOfString;
  Search: Cardinal;
  Data: TGuardFindData;
  Name, Child: String;
  Attributes: DWORD;
  Position, Directories, Entries, Code: Integer;
begin
  SetArrayLength(Pending, GuardMaximumEntries + 1);
  Pending[0] := Root;
  Position := 0;
  Directories := 1;
  Entries := 0;
  while Position < Directories do begin
    Search := GuardFindFirst('\\?\' + Pending[Position] + '\*', Data);
    if Search = Cardinal(-1) then begin
      Code := DLLGetLastError;
      if Code <> GuardFileNotFound then GuardFail('Cannot enumerate installation directory', Pending[Position], Code);
    end else begin
      try
        repeat
          Name := GuardFindName(Data);
          if (Name <> '.') and (Name <> '..') then begin
            Entries := Entries + 1;
            if (Name = '') or (Entries > GuardMaximumEntries) then
              GuardFail('Installation tree has unexpected contents', Root, 0);
            Child := Pending[Position] + '\' + Name;
            if not GuardReadEntry(Child, Users, False, Attributes) then
              GuardFail('Installation entry changed during verification', Child, 0);
            if (Attributes and GuardDirectoryAttribute) <> 0 then begin
              Pending[Directories] := Child;
              Directories := Directories + 1;
            end;
          end;
        until GuardFindNext(Search, Data) = 0;
        Code := DLLGetLastError;
        if Code <> GuardNoMoreFiles then GuardFail('Cannot finish installation directory enumeration', Pending[Position], Code);
      finally
        GuardFindClose(Search);
      end;
    end;
    Position := Position + 1;
  end;
end;

function GuardCheckInstallPath(const Action: String; const Users: TGuardUsers): Boolean;
var
  Parent, Relative, Path, Name: String;
  Attributes: DWORD;
  Separator: Integer;
begin
  Result := False;
  Parent := ExpandConstant('{autopf}');
  Path := InstallPath;
  if (Length(Path) <= Length(Parent) + 1) or
    (CompareText(Copy(Path, 1, Length(Parent) + 1), Parent + '\') <> 0) then
    GuardFail('Installation must be inside Program Files', Path, 0);
  if not GuardReadEntry(Parent, Users, True, Attributes) or
    ((Attributes and GuardDirectoryAttribute) = 0) then
    GuardFail('Expected Program Files directory', Parent, 0);

  Relative := Copy(Path, Length(Parent) + 2, Length(Path));
  Path := Parent;
  while Relative <> '' do begin
    Separator := Pos('\', Relative);
    if Separator = 0 then Separator := Length(Relative) + 1;
    Name := Copy(Relative, 1, Separator - 1);
    if (Name = '') or (Name = '.') or (Name = '..') then
      GuardFail('Invalid installation directory component', Relative, 0);
    Delete(Relative, 1, Separator);
    Path := Path + '\' + Name;
    if not GuardReadEntry(Path, Users, False, Attributes) then begin
      if Action <> 'Prepare' then begin
        Result := True;
        Exit;
      end;
      GuardPrepareDirectory(Path, True);
      if not GuardReadEntry(Path, Users, False, Attributes) then
        GuardFail('Protected installation directory was not created', Path, 0);
    end;
    if (Attributes and GuardDirectoryAttribute) = 0 then
      GuardFail('Expected an installation directory', Path, 0);
  end;
  GuardCheckTree(Path, Users);
  Result := True;
end;

function GuardInstall(const Action: String): Boolean;
var
  Users: TGuardUsers;
  Image: String;
  Code: Integer;
begin
  Result := False;
  if not FixedInstallPath or ((Action <> 'Check') and (Action <> 'Prepare') and (Action <> 'Stop')) then Exit;
  Users.Administrators := 0;
  Users.LocalSystem := 0;
  Users.TrustedInstaller := 0;
  try
    try
      if (GuardConvertSid('S-1-5-32-544', Users.Administrators) = 0) or
        (GuardConvertSid('S-1-5-18', Users.LocalSystem) = 0) or
        (GuardConvertSid('S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464',
          Users.TrustedInstaller) = 0) then Exit;
      Result := GuardCheckInstallPath(Action, Users);
      if Result and (Action = 'Stop') then begin
        Image := InstallPath + '\Prismestra.exe';
        if FileExists(Image) then
          Result := Exec(Image, '--shutdown-installed', InstallPath, SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
      end;
    except
      Log(GetExceptionMessage);
      Result := False;
    end;
  finally
    if Users.TrustedInstaller <> 0 then GuardFreeLocal(Users.TrustedInstaller);
    if Users.LocalSystem <> 0 then GuardFreeLocal(Users.LocalSystem);
    if Users.Administrators <> 0 then GuardFreeLocal(Users.Administrators);
  end;
end;
