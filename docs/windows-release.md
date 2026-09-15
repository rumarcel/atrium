# Windows builds and releases

Run `pnpm install --frozen-lockfile` and `pnpm build:windows` on a Windows machine
with the documented Tauri prerequisites. The resulting per-user installer is
under `src-tauri/target/release/bundle/nsis/`. It offers English/Turkish, uses the
bundled application icon and installs WebView2 via Microsoft's bootstrapper if
needed. Internet access is needed if the build tools or WebView2 are absent.

The repository's manually triggered **Windows installer** GitHub workflow builds
the same unsigned artifact. It does not publish a release or upload certificates.
Keep `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` versions
in sync when preparing a new version; commit both lockfiles with dependency changes.

## Signing

Ordinary local/workflow builds are unsigned. Atrium does not claim that a
binary's signature has been verified; Settings displays the actual running version
and build profile, with signature status explicitly unverified.

To make a signed release, provision your own Windows code-signing certificate and
Windows SDK SignTool. Copy `src-tauri/tauri.signing.conf.example.json` to the ignored
`src-tauri/tauri.signing.conf.json`, replace its placeholder with that certificate's
thumbprint, then run:

```powershell
pnpm tauri build --bundles nsis --config src-tauri/tauri.signing.conf.json
```

Use a timestamp server supported by your certificate provider. The example follows
Tauri's [Windows signing documentation](https://v2.tauri.app/distribute/sign/windows/).
Never commit certificate/private-key files. A signed production release still
requires a real certificate; this repository does not supply one.

Verify **both** the bundled application EXE and installer using Windows file
properties or `Get-AuthenticodeSignature` before distributing a signed release.

## Targeted installed-build smoke check

- Install without Administrator mode; verify icon, Start menu launch and About version.
- Opt into startup from Settings, relaunch, check the switch, then turn it off.
  Development builds cannot register their temporary executable for startup.
- Launch a second time while hidden in the tray: the existing main window should
  restore. Quit from the tray exits the whole process.
- Check main-window size/position/maximization across relaunch and a disconnected
  monitor. Hidden/fullscreen state and experimental-card geometry are not restored
  by the main-window persistence plugin.
- Enable only the notification categories you want. Toast delivery requires an
  installed Windows app and may be suppressed by Windows notification settings.
  The first observation is silent; persistent outages or high disk use can alert
  on later checks. Already-completed downloads never produce historical alerts.
- Test an outage/recovery, disk threshold crossing and one observed download
  completion. Deleting a download must never be reported as completion.

This smoke checklist is for the installed release environment; compilation and
targeted unit checks cannot establish Windows toast delivery or code-signing trust.
