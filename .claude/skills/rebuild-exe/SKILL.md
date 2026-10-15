---
name: rebuild-exe
description: Rebuild the NovaMix Windows executables (novamix.exe, novamix-cli.exe) and the dist\ package. Use after ANY change to the program — Rust code in app/ or crates/, Cargo.toml/Cargo.lock, build.rs, icon, i18n strings, docs shipped in dist\ or packaging scripts — before telling the user the change is done. Also use when the user asks to build, rebuild or generate the .exe.
---

# Rebuild the NovaMix .exe

Every change to the program must end with a fresh release build, so `dist\NovaMix\novamix.exe`
always matches the source. Run this after the code change is finished (not after every
intermediate edit) and before reporting the work as done.

## 1. Make sure the old exe is not running

The linker cannot overwrite a running executable ("Access is denied" / "os error 5").

```powershell
Get-Process novamix, novamix-cli -ErrorAction SilentlyContinue | Select-Object Name, Id, Path
```

If NovaMix is running, **ask the user** before closing it (it may be routing their live audio).
Only after they agree: `Stop-Process -Name novamix -Confirm:$false`.

## 2. Build

Full pipeline (tests + release build + portable zip + installer if Inno Setup exists + checksums):

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\build-release.ps1
```

- Takes several minutes: run it with `run_in_background` and wait for the notification.
- If the tests were already run on the final code in this session, add `-SkipTests`.
- Warnings about `NOVAMIX_SIGN_CERT` (unsigned binaries) and a missing driver package are
  expected on this machine; they are not failures.

If the script fails, read the error, fix the cause and rebuild. Never report a change as done
with a failing build or failing tests.

## 3. Verify

```powershell
Get-Item dist\NovaMix\novamix.exe, dist\NovaMix\novamix-cli.exe | Select-Object Name, Length, LastWriteTime
(Get-Item dist\NovaMix\novamix.exe).VersionInfo.ProductVersion
.\dist\NovaMix\novamix-cli.exe --help
```

- `LastWriteTime` must be from this build (newer than the source change).
- `ProductVersion` of `novamix.exe` must match `version` in the root `Cargo.toml`
  (`novamix-cli.exe` has no version resource; empty is expected).
- Optional smoke test without touching audio: start `dist\NovaMix\novamix.exe --no-audio`,
  check the window title is `NovaMix` after a few seconds, then close it.

## 4. Report

Tell the user (in pt-BR) where the artifacts are and whether tests ran:

- `dist\NovaMix\novamix.exe` — desktop app (no console window in release)
- `dist\NovaMix\novamix-cli.exe` — CLI
- `dist\NovaMix-<version>-portable.zip`, `dist\NovaMix-<version>-setup.exe` (if Inno Setup is installed),
  `dist\SHA256SUMS.txt`

If the change is user-visible, consider bumping `version` in `Cargo.toml` and adding a
`CHANGELOG.md` entry — ask the user before bumping the version.
