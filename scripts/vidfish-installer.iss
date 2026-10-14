; Per-user installer: no UAC, unsigned unless release signing is configured.
[Setup]
AppId=VidfishScientificAcquisition
AppName=Vidfish
AppVersion={#ReleaseVersion}
AppPublisher=Matjordan
AppPublisherURL=https://github.com/Matjordan/vidfish
DefaultDirName={localappdata}\Programs\Vidfish
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\share\icons\vidfish.ico
SetupIconFile=..\assets\vidfish.ico
OutputDir=..
OutputBaseFilename=vidfish-{#ReleaseVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
CloseApplications=no
RestartApplications=no
[Files]
Source: "{#BundleRoot}\*"; DestDir: "{tmp}\vidfish-bundle"; Flags: recursesubdirs createallsubdirs deleteafterinstall
; A dedicated bootstrap remains available for uninstall without touching app data.
Source: "{#BundleRoot}\vidfish-maintenance.exe"; DestDir: "{app}\native"; Flags: ignoreversion
Source: "{#BundleRoot}\*.dll"; DestDir: "{app}\native"; Flags: ignoreversion
[Icons]
Name: "{userprograms}\Vidfish"; Filename: "{app}\bin\vidfish.cmd"; IconFilename: "{app}\share\icons\vidfish.ico"; WorkingDir: "{localappdata}"
[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
end;
procedure CurStepChanged(CurStep: TSetupStep);
var ExitCode: Integer;
begin
  if CurStep = ssPostInstall then begin
    if not Exec(ExpandConstant('{tmp}\vidfish-bundle\vidfish-maintenance.exe'),
      'install --prefix "' + ExpandConstant('{app}') + '" --bundle "' + ExpandConstant('{tmp}\vidfish-bundle') + '"',
      '', SW_HIDE, ewWaitUntilTerminated, ExitCode) or (ExitCode <> 0) then
      RaiseException('Vidfish installation could not be completed. Close Vidfish and its recording services, check free space and write access, then retry. Existing recordings and settings are retained.');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var ExitCode: Integer;
begin
  if CurUninstallStep = usUninstall then begin
    if not Exec(ExpandConstant('{app}\native\vidfish-maintenance.exe'),
      'uninstall --prefix "' + ExpandConstant('{app}') + '"', '', SW_HIDE,
      ewWaitUntilTerminated, ExitCode) or (ExitCode <> 0) then
      RaiseException('Close Vidfish and recording services before uninstalling. No scientific data was removed.');
  end;
end;
