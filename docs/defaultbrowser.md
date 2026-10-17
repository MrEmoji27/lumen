# Lumen as the default browser

Windows 10 and 11 do not let a program make itself the default. A program may only describe
itself; the person picks it in Settings, and Windows records the pick in a `UserChoice` key that
is protected by a hash. Lumen follows those rules: it writes only what describes it, never
`UserChoice`, and never `HKEY_LOCAL_MACHINE`. Everything is under `HKEY_CURRENT_USER`.

## Making it default

- Settings > General > **Make Lumen your default browser**. Writes the entries below (if they
  are missing) and opens `ms-settings:defaultapps?registeredAppUser=Lumen`, where Windows asks
  the person to pick Lumen. The row shows the state: *Lumen is your default browser* when
  `...\Shell\Associations\UrlAssociations\https\UserChoice\ProgId` is `LumenHTML`, *Not default*
  otherwise.
- `lumen.exe --register` does the same without the window (for scripts).
- **Unregister** (Settings > General, or `lumen.exe --unregister`) removes exactly what Lumen
  wrote, and nothing else. Windows keeps the person's own pick; only Windows can change that.
- At startup Lumen re-writes the entries only if they exist already and the exe has moved
  (an update in a new folder). It never registers a Lumen that was never registered.

## What it writes (all under HKCU)

```
HKCU\Software\Classes\LumenHTML
    (Default)              = "Lumen HTML Document"
    DefaultIcon            = "<exe>",0
    shell\open\command     = "<exe>" --single-argument %1
    Application\ApplicationName        = Lumen
    Application\ApplicationDescription = A small, fast browser for Windows.
    Application\ApplicationIcon        = "<exe>",0

HKCU\Software\Clients\StartMenuInternet\Lumen
    (Default)              = Lumen
    DefaultIcon            = "<exe>",0
    shell\open\command     = "<exe>"
    Capabilities\ApplicationName        = Lumen
    Capabilities\ApplicationDescription = A small, fast browser for Windows.
    Capabilities\ApplicationIcon        = "<exe>",0
    Capabilities\URLAssociations\http   = LumenHTML
    Capabilities\URLAssociations\https  = LumenHTML
    Capabilities\FileAssociations\.htm  = LumenHTML     (also .html .shtml .xht .xhtml
                                                         .svg .pdf .webp)

HKCU\Software\RegisteredApplications
    Lumen                  = Software\Clients\StartMenuInternet\Lumen\Capabilities
```

`--single-argument` in the open command is the security-relevant part: Windows appends the link
or file as `%1`, and Lumen takes everything after that flag as ONE address, never as options.
A crafted link such as `https://x/ --headless` is a URL with a space in it, not a URL plus a
flag. The registry is also why the command is quoted: the exe path contains spaces.

Nothing else is written, and no key is ever removed except the three above.

## The command line

```
lumen.exe <url-or-path>...      opens each as a tab (http/https, .html .htm .pdf .svg .xhtml .mht)
lumen.exe --private <url>       opens it as a private tab
lumen.exe --single-argument <one address>   what Windows' registry command uses
lumen.exe --register | --unregister         the entries above, no window
```

If a Lumen is already running for the same profile folder, a second start hands it the addresses
through the single-instance channel and leaves; the running one opens the tabs (and comes to the
front, unless the start was `--headless`). Command lines are re-checked on the receiving side, so
what a second start opens is exactly what a first start would: http(s) URLs and page-like files,
never `javascript:`, `data:`, `file:` of something executable, or any other scheme.

## Other apps' links (mailto:, spotify:, steam:...)

A page asking to open an app is a question for the person. WebView2 raises
`LaunchingExternalUriScheme`; Lumen cancels WebView2's own box and asks with its own dialog:
**Open / Always for this site / Cancel**. "Always" is kept in the same permission rules store as
camera and location answers (`open <scheme>` for that origin), visible under Settings > Site
permissions, where a Reset takes it back. Agent tabs never open anything.

Screenshots: `docs/ui-shots/defaultbrowser-general.png`.
