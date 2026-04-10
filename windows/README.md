# YT HOME RUST Windows Files

This directory contains all Windows-specific files for YT HOME RUST.

## Available Files

- **s-ui-windows.xml**: Windows Service configuration
- **install-windows.bat**: Installation script
- **s-ui-windows.bat**: Control panel
- **uninstall-windows.bat**: Uninstallation script
- **build-windows.bat**: CMD wrapper around the PowerShell build script
- **build-windows.ps1**: Rust + frontend + sing-box bundle builder

## Usage

To install YT HOME RUST on Windows:
1. Run `install-windows.bat` as Administrator
2. Follow the installation wizard
3. Use `s-ui-windows.bat` for management

To build from source:
- With CMD: `build-windows.bat amd64`
- With PowerShell: `.\build-windows.ps1 -Architecture amd64`

The build output is written to `windows/dist/<arch>/` and includes:

- `sui.exe`
- `sing-box.exe`
- `libcronet.dll` when the upstream asset provides it
- built frontend assets under `web/`
- SQLite migrations under `migrations/`
- Windows helper scripts for install and uninstall
