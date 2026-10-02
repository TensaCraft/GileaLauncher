# Privacy

GileaLauncher is a Minecraft launcher that runs on your computer. It has no accounts of its own, no ads, no analytics and no tracking. This page says what it keeps, what it sends and where.

## What stays on your computer

- **Settings, builds, worlds and mods** are kept in the launcher's folders (see "Where data is kept" in the [README](README.md#where-data-is-kept)).
- **Accounts.** For a Microsoft account the launcher keeps your player name, UUID and the sign-in tokens Microsoft gives it. The tokens are encrypted, and the key sits in the same folder. This keeps them unreadable in a copied or shared file, not from someone who can use your computer account. Offline profiles keep only a name.
- **Logs** of the launcher and of the game stay in those folders until you delete them.

Nothing of this leaves your computer unless a step below says so.

## Who the launcher talks to

The launcher connects to these services only to do what you ask of it. Each one sees your IP address and the launcher's name and version, as any download does.

| Service | Why | What it receives |
|---|---|---|
| Microsoft, Xbox Live, Minecraft services | Signing in with a Microsoft account, checking that you own the game | The sign-in happens on Microsoft's own page, in your browser; the launcher then exchanges the tokens Microsoft gives it |
| Mojang (`piston-meta.mojang.com`, `libraries.minecraft.net`, `resources.download.minecraft.net`) | Minecraft versions, their files and Java for the game | Which files to download |
| Fabric, Quilt, Forge, NeoForge | Loader versions and their files | Which files to download |
| Modrinth, CurseForge | Searching and installing mods, resource packs, shaders and modpacks, checking for updates | Your searches, and fingerprints (hashes) of the files in a build when the launcher looks for their updates |
| GitHub | Checking for and downloading launcher updates | Nothing beyond the request |
| mc-heads.net, minotar.net, mineskin.eu | The skin heads shown next to your accounts | The player's UUID or name |
| `gigabait.uk` (TensaCraft edition only) | The TensaCraft servers' builds on Home | Nothing beyond the request |

When you start the game, Minecraft itself talks to Mojang and Microsoft under [their privacy statement](https://privacy.microsoft.com/privacystatement).

## Reports

When the game or the launcher fails, the error window has a "Send report" button. A report is sent only when you press it, to `gigabait.uk`, so that the problem can be found and fixed.

A report holds:
- the error's title and message;
- the launcher's version, the system (OS) and the launcher's recent activity;
- for a build: its name, Minecraft and loader version, Java path, memory, Java arguments and server address;
- the logs: the launcher's log and the game's latest log, crash report and Java crash log;
- your contact, only if you entered one.

Before a report is sent, the launcher replaces your home folder's path with `<USER_HOME>` and removes sign-in tokens from the logs. The game's logs can still contain your player name.

To have a report you sent deleted, ask on [Discord](https://discord.com/invite/mftAjQA4Pp).

## Deleting your data

- Delete an account on the launcher's Profiles page to delete its tokens.
- Delete the launcher's folders to delete everything it keeps.

## Contact

Questions about this page: [Discord](https://discord.com/invite/mftAjQA4Pp) or [GitHub Issues](https://github.com/TensaCraft/GileaLauncher/issues).

Last updated: 2 October 2026.
