<p align="center">
  <img src="docs/screenshots/hero.jpg" alt="GileaLauncher — a modern Minecraft launcher">
</p>

<p align="center">
  A Minecraft launcher for Windows, Linux and macOS.<br>
  <a href="https://github.com/TensaCraft/GileaLauncher/releases/latest"><b>Download</b></a> ·
  <a href="https://discord.com/invite/mftAjQA4Pp">Discord</a>
</p>

## Features

- Installs any Minecraft version, plus Forge, NeoForge, Fabric and Quilt.
- Modrinth modpacks and mods: search, install, update.
- CurseForge modpacks, mods, resource packs and shaders: search, install with dependencies, update.
- Several builds side by side, each with its own worlds, mods and settings.
- Continue playing on Home: each recent build's last server, with its MOTD, players and ping, or world, one click back in.
- Microsoft accounts and offline profiles; a build can have an account of its own.
- Java for the game is picked and downloaded automatically.
- World backups.
- Desktop shortcut for a build.

## Screenshots

<table>
  <tr>
    <td><img src="docs/screenshots/home.jpg" alt="Home"><br><sub>Home: your builds, one click to play</sub></td>
    <td><img src="docs/screenshots/builds.jpg" alt="Builds"><br><sub>Builds side by side</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/installed-mods.jpg" alt="Installed mods"><br><sub>A build's mods, with their updates</sub></td>
    <td><img src="docs/screenshots/mod-search.jpg" alt="Mod search"><br><sub>Mods from Modrinth and CurseForge</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/modpacks.jpg" alt="Modpacks"><br><sub>Modpacks, installed as builds</sub></td>
    <td><img src="docs/screenshots/create-build.jpg" alt="Create a build"><br><sub>Any Minecraft version, with its loader</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/settings.jpg" alt="Settings"><br><sub>Settings</sub></td>
    <td><img src="docs/screenshots/continue-playing.jpg" alt="Continue playing"><br><sub>Continue playing: your last server or world, one click back in</sub></td>
  </tr>
</table>

## Editions

| Edition | What's inside | Files |
|---|---|---|
| **GileaLauncher** | everything above | `GileaLauncher-standard…` |
| **GileaLauncher + TensaCraft** | the same, plus TensaCraft server builds on Home | `GileaLauncher-tensa…` |

Each edition updates only to a newer version of the same edition. To switch editions, download the other edition's file. Both editions share settings, profiles and builds, so nothing is lost.

## Installation

Get the files from the [Releases](https://github.com/TensaCraft/GileaLauncher/releases/latest) page. Below, `standard` is the edition name; for TensaCraft it is `tensa`.

### Windows 10 and 11

- **`GileaLauncher-standard-Setup.exe`** — installer. No administrator rights needed. Adds the launcher to the Start menu.
- **`GileaLauncher-standard.exe`** — portable version, no installation: just run it.

The launcher is not signed yet, so Windows may show "Windows protected your PC". Click "More info", then "Run anyway".

### Linux (x86_64)

- **`GileaLauncher-standard-x86_64.AppImage`** — recommended. Everything it needs is inside, so there is nothing to install. Make it executable and run it:
  ```bash
  chmod +x GileaLauncher-standard-x86_64.AppImage
  ./GileaLauncher-standard-x86_64.AppImage
  ```
  In a file manager you can do the same: open the file's properties, allow running it as a program, then double-click it. On a minimal system without the `fusermount` tool, the AppImage says so; start it with `--appimage-extract-and-run` instead.
- **`GileaLauncher-standard-x86_64`** — a smaller plain executable that uses the system's WebKitGTK 4.1, which is often not installed by default (on Ubuntu: `sudo apt install libwebkit2gtk-4.1-0`).

### macOS

**`GileaLauncher-standard-universal.dmg`** — for Apple Silicon and Intel. Open the image and drag GileaLauncher to Applications.

The launcher is not notarized by Apple yet, so macOS blocks the first launch. Open System Settings → Privacy & Security and click "Open Anyway" next to the message about GileaLauncher. On macOS 14 and older, Control-click GileaLauncher in Applications and choose "Open". If macOS says the app is damaged, run in Terminal:

```bash
xattr -dr com.apple.quarantine /Applications/GileaLauncher.app
```

## Updates

The launcher checks for new versions of its edition and offers to update. To get beta versions, turn on "Beta updates" in the launcher's settings. Updates never touch your data.

## Where data is kept

On first launch, the setup wizard shows where settings and games will be kept, and lets you choose another folder. Defaults:

| System | Settings | Games |
|---|---|---|
| Windows | `%LOCALAPPDATA%\GileaLauncher` | `%APPDATA%\GileaLauncher` |
| Linux | `~/.config/GileaLauncher` | `~/.local/share/GileaLauncher` |
| macOS | `~/Library/Application Support/GileaLauncher` | `~/Library/Application Support/GileaLauncher/minecraft` |

## Support

- Questions and help: [Discord](https://discord.com/invite/mftAjQA4Pp).
- If the game crashes, click "Send report" in the error window.
- Bugs and suggestions: [Issues](https://github.com/TensaCraft/GileaLauncher/issues).

What the launcher keeps and sends: [PRIVACY.md](PRIVACY.md).

For developers: [docs/](docs/README.md).

## License

[MIT](LICENSE) © GIGABAIT. The Inter, Exo 2 and JetBrains Mono fonts are under SIL OFL 1.1, Material Icons under Apache 2.0 ([assets/fonts/licenses](assets/fonts/licenses)).

GileaLauncher is an unofficial launcher, not affiliated with Mojang or Microsoft. Minecraft is a trademark of Mojang AB.

CurseForge works in the official builds only: CurseForge issued an API key for this launcher, and the release builds get it from a secret. A fork or your own build needs its own key ([apply to CurseForge](https://console.curseforge.com/)), set as `CURSEFORGE_API_KEY` when building or running; without one CurseForge is hidden. GileaLauncher is not affiliated with CurseForge or Overwolf.
