<p align="right"><b>English</b> · <a href="README.uk.md">Українська</a></p>

<p align="center">
  <img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/hero.jpg" alt="GileaLauncher — a modern Minecraft launcher">
</p>

<p align="center">
  A Minecraft launcher for Windows, Linux and macOS.<br>
  <a href="#download"><b>Download</b></a> ·
  <a href="https://discord.com/invite/mftAjQA4Pp">Discord</a>
</p>

<p align="center">
  <a href="https://github.com/TensaCraft/GileaLauncher/releases"><img src="https://img.shields.io/github/downloads/TensaCraft/GileaLauncher/total?label=downloads&color=2ea44f" alt="Downloads"></a>
  <a href="https://github.com/TensaCraft/GileaLauncher/releases/latest"><img src="https://img.shields.io/github/v/release/TensaCraft/GileaLauncher?label=version" alt="Latest version"></a>
  <a href="https://github.com/TensaCraft/GileaLauncher/stargazers"><img src="https://img.shields.io/github/stars/TensaCraft/GileaLauncher?style=flat&color=yellow" alt="Stars"></a>
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#screenshots">Screenshots</a> ·
  <a href="#design-variants">Design variants</a> ·
  <a href="#installation">Installation</a> ·
  <a href="#where-data-is-kept">Data and privacy</a> ·
  <a href="#support">Support</a>
</p>

## Download

The launcher comes in two editions. Pick one: the links always lead to the latest version.

### GileaLauncher

The standard edition: everything described below, with Modrinth and CurseForge.

| System | File | |
|---|---|---|
| Windows 10 and 11 | Installer, no administrator rights needed<br>`GileaLauncher-standard-Setup.exe` | [**Download**](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-standard-Setup.exe) |
| Windows 10 and 11 | Portable, runs without installing<br>`GileaLauncher-standard.exe` | [Download](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-standard.exe) |
| Linux x86_64 | AppImage, everything inside<br>`GileaLauncher-standard-x86_64.AppImage` | [**Download**](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-standard-x86_64.AppImage) |
| Linux x86_64 | Executable that uses the system's WebKitGTK 4.1<br>`GileaLauncher-standard-x86_64` | [Download](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-standard-x86_64) |
| macOS | Disk image for Apple Silicon and Intel<br>`GileaLauncher-standard-universal.dmg` | [**Download**](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-standard-universal.dmg) |

### GileaLauncher + TensaCraft

The TensaCraft edition: the same launcher, plus the TensaCraft servers' builds on Home.

| System | File | |
|---|---|---|
| Windows 10 and 11 | Installer, no administrator rights needed<br>`GileaLauncher-tensa-Setup.exe` | [**Download**](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-tensa-Setup.exe) |
| Windows 10 and 11 | Portable, runs without installing<br>`GileaLauncher-tensa.exe` | [Download](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-tensa.exe) |
| Linux x86_64 | AppImage, everything inside<br>`GileaLauncher-tensa-x86_64.AppImage` | [**Download**](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-tensa-x86_64.AppImage) |
| Linux x86_64 | Executable that uses the system's WebKitGTK 4.1<br>`GileaLauncher-tensa-x86_64` | [Download](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-tensa-x86_64) |
| macOS | Disk image for Apple Silicon and Intel<br>`GileaLauncher-tensa-universal.dmg` | [**Download**](https://github.com/TensaCraft/GileaLauncher/releases/latest/download/GileaLauncher-tensa-universal.dmg) |

Each edition updates only to a newer version of the same edition. To switch editions, download the other edition's file. Both editions share settings, profiles and builds, so nothing is lost. Every version and its changes: [Releases](https://github.com/TensaCraft/GileaLauncher/releases).

The first launch on each system has a step of its own: see [Installation](#installation).

## Features

- **Any Minecraft version**, plus Forge, NeoForge, Fabric and Quilt.
- **Modrinth and CurseForge:** modpacks, mods, resource packs and shaders. Search, install with dependencies, update.
- **Builds side by side**, each with its own worlds, mods and settings.
- **Continue playing** on Home: each recent build's last server, with its MOTD, players and ping, or world, one click back in.
- **Accounts:** Microsoft accounts and offline profiles; a build can have an account of its own.
- **Java** for the game is picked and downloaded automatically.
- **World backups** and a **desktop shortcut** for a build.
- **Your own look:** Home, the Play button and the sidebar each come in a few [variants](#design-variants).
- English and Ukrainian interface.

## Screenshots

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/continue-playing.jpg" alt="Home with Continue playing"></p>
<p align="center"><b>Home.</b> Continue playing: your last server or world, one click back in.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/builds.jpg" alt="Builds"></p>
<p align="center"><b>Builds</b> side by side, each with its own worlds, mods and settings.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/installed-mods.jpg" alt="A build's mods"></p>
<p align="center"><b>A build's mods,</b> with their updates.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/mod-search.jpg" alt="Mod search"></p>
<p align="center"><b>Mods from Modrinth and CurseForge,</b> for the build's version and loader.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/modpacks.jpg" alt="Modpacks"></p>
<p align="center"><b>Modpacks,</b> installed as builds.</p>

<details>
<summary><b>More screenshots</b></summary>
<br>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/create-build.jpg" alt="Create a build"></p>
<p align="center"><b>Create a build:</b> any Minecraft version, with its loader.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/shaders.jpg" alt="Shaders"></p>
<p align="center"><b>Shaders</b> for a build.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/resource-packs.jpg" alt="Resource packs"></p>
<p align="center"><b>Resource packs</b> for a build.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/home.jpg" alt="Home with just the builds"></p>
<p align="center"><b>Home</b> with just the builds.</p>

<p align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/readme/en_US/settings.jpg" alt="Settings"></p>
<p align="center"><b>Settings.</b></p>

</details>

## Design variants

Choose them in the first-run wizard, or any time later in Settings → Interface.

**Home**

<table>
  <tr>
    <td width="50%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/home-recent.jpg" alt="Home: Continue playing"><br><b>Continue playing</b><br><sub>Your last builds with the server or world you played on.</sub></td>
    <td width="50%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/home-builds.jpg" alt="Home: just builds"><br><b>Just builds</b><br><sub>Only the build cards, no history.</sub></td>
  </tr>
</table>

**Play button on cards**

<table>
  <tr>
    <td width="33%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/cards-center.jpg" alt="Play button: round, in the middle"><br><b>Round, in the middle</b><br><sub>Shows when you point at a card.</sub></td>
    <td width="33%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/cards-bar.jpg" alt="Play button: a bar at the bottom"><br><b>A bar at the bottom</b><br><sub>Takes the version line's place.</sub></td>
    <td width="33%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/cards-corner.jpg" alt="Play button: in the corner"><br><b>In the corner</b><br><sub>A small button, always shown.</sub></td>
  </tr>
</table>

**Sidebar**

<table>
  <tr>
    <td width="50%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/sidebar-compact.jpg" alt="Sidebar: icons only"><br><b>Icons only</b><br><sub>A narrow menu, section names in tooltips.</sub></td>
    <td width="50%" align="center"><img src="https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/setup/en_US/sidebar-full.jpg" alt="Sidebar: with labels"><br><b>With labels</b><br><sub>A wider menu with section names.</sub></td>
  </tr>
</table>

## Installation

Below, `standard` is the edition name; for TensaCraft it is `tensa`.

### Windows 10 and 11

The installer adds the launcher to the Start menu; the portable `.exe` just runs.

The launcher is not signed yet, so Windows may show "Windows protected your PC". Click "More info", then "Run anyway".

### Linux (x86_64)

The AppImage is the recommended file: everything it needs is inside, so there is nothing to install. Make it executable and run it:

```bash
chmod +x GileaLauncher-standard-x86_64.AppImage
./GileaLauncher-standard-x86_64.AppImage
```

In a file manager you can do the same: open the file's properties, allow running it as a program, then double-click it. On a minimal system without the `fusermount` tool, the AppImage says so; start it with `--appimage-extract-and-run` instead.

The smaller plain executable, `GileaLauncher-standard-x86_64`, uses the system's WebKitGTK 4.1, which is often not installed by default (on Ubuntu: `sudo apt install libwebkit2gtk-4.1-0`).

### macOS

Open the disk image and drag GileaLauncher to Applications.

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

**The Windows registry.** By default the launcher writes nothing to the Windows registry (GPU mode Auto). Only if you choose an integrated or discrete GPU does it write that choice for the game's Java to `HKEY_CURRENT_USER`, the same place Windows Settings → Graphics writes to, and it asks you first; switching back to Auto removes it. [Details](docs/en/privacy.md#the-windows-registry).

What the launcher keeps and sends, and who it talks to: [docs/en/privacy.md](docs/en/privacy.md).

## Support

- Questions and help: [Discord](https://discord.com/invite/mftAjQA4Pp).
- If the game crashes, its error window opens the crash report and the game's log; most often a build's mods are the cause.
- A problem with the launcher itself: "Report" on the failure's notice, or Support → "A problem with the launcher". The report carries the launcher's own log, never the game's.
- Bugs and suggestions: [Issues](https://github.com/TensaCraft/GileaLauncher/issues).

For developers: [docs/](docs/en/README.md).

## License

[MIT](LICENSE) © GIGABAIT. The Inter, Exo 2 and JetBrains Mono fonts are under SIL OFL 1.1, Material Icons under Apache 2.0 ([assets/fonts/licenses](assets/fonts/licenses)).

GileaLauncher is an unofficial launcher, not affiliated with Mojang or Microsoft. Minecraft is a trademark of Mojang AB.

CurseForge works in the official builds only: CurseForge issued an API key for this launcher, and the release builds get it from a secret. A fork or your own build needs its own key ([apply to CurseForge](https://console.curseforge.com/)), set as `CURSEFORGE_API_KEY` when building or running; without one CurseForge is hidden. GileaLauncher is not affiliated with CurseForge or Overwolf.
