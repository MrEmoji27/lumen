; Lumen's installer. Built by tools\package.ps1, which passes /DVersion and /DStage (the
; folder holding lumen.exe, lumen-agent.exe and README.txt). By hand:
;   "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" /DVersion=0.6.7 /DStage=C:\...\dist\lumen-0.6.7 /Odist tools\lumen.iss
;
; Per user, no admin: %LOCALAPPDATA%\Programs\Lumen. A Start menu entry. The uninstaller
; leaves the person's data (%APPDATA%\Lumen) unless they say to delete it. When WebView2 is
; missing, Microsoft's Evergreen bootstrapper is fetched and run.

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Stage
  #error Pass /DStage=<folder with lumen.exe>
#endif

[Setup]
AppId={{6E2C2B1A-5D7F-4C1B-9A3E-4B8F2D6C1A70}
AppName=Lumen
AppVersion={#Version}
AppPublisher=zemo
AppPublisherURL=https://portfolio.mremoji47.workers.dev/
DefaultDirName={localappdata}\Programs\Lumen
DefaultGroupName=Lumen
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputBaseFilename=lumen-{#Version}-setup
SetupIconFile=..\assets\lumen.ico
UninstallDisplayIcon={app}\lumen.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Files]
Source: "{#Stage}\lumen.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\lumen-agent.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.txt"; DestDir: "{app}"; Flags: ignoreversion isreadme

[Icons]
Name: "{userprograms}\Lumen"; Filename: "{app}\lumen.exe"
Name: "{userdesktop}\Lumen"; Filename: "{app}\lumen.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "A desktop shortcut"; Flags: unchecked

[Run]
Filename: "{app}\lumen.exe"; Description: "Start Lumen"; Flags: nowait postinstall skipifsilent

[Code]
const
  WebView2Client = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  Bootstrapper = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';

{ The WebView2 Runtime is here: machine-wide or for this user, with a real version. }
function HasWebView2(): Boolean;
var
  V: String;
begin
  Result :=
    (RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\' + WebView2Client, 'pv', V) and (V <> '') and (V <> '0.0.0.0')) or
    (RegQueryStringValue(HKLM, 'SOFTWARE\' + WebView2Client, 'pv', V) and (V <> '') and (V <> '0.0.0.0')) or
    (RegQueryStringValue(HKCU, 'Software\' + WebView2Client, 'pv', V) and (V <> '') and (V <> '0.0.0.0'));
end;

{ After the files: no WebView2, fetch Microsoft's bootstrapper and let it install the runtime. }
procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if (CurStep = ssPostInstall) and not HasWebView2() then
  begin
    WizardForm.StatusLabel.Caption := 'Installing Microsoft WebView2...';
    try
      DownloadTemporaryFile(Bootstrapper, 'MicrosoftEdgeWebview2Setup.exe', '', nil);
      Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe'), '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, Code);
    except
      MsgBox('WebView2 could not be downloaded. Lumen will offer the download page when it starts.', mbInformation, MB_OK);
    end;
  end;
end;

{ The person's data stays unless they say otherwise. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent() then
    if MsgBox('Also delete your Lumen data (settings, history, bookmarks)?' #13#10 + ExpandConstant('{userappdata}\Lumen'),
              mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{userappdata}\Lumen'), True, True, True);
end;
