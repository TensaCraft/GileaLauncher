<p align="right"><b>English</b> · <a href="../uk/privacy.md">Українська</a></p>

# Privacy

GileaLauncher is a Minecraft launcher that runs on your computer. It has no accounts of its own, no ads, no analytics and no tracking. This page says what it keeps, what it sends and where.

## What stays on your computer

- **Settings, builds, worlds and mods** are kept in the launcher's folders (see "Where data is kept" in the [README](../../README.md#where-data-is-kept)).
- **Accounts.** For a Microsoft account the launcher keeps your player name, UUID and the sign-in tokens Microsoft gives it. The tokens are encrypted, and the key sits in the same folder. This keeps them unreadable in a copied or shared file, not from someone who can use your computer account. Offline profiles keep only a name.
- **Logs** of the launcher and of the game stay in those folders until you delete them.
- **The launcher's own crashes.** If the launcher crashes, it keeps a short note (the error's text and its place in the launcher's code, without your home folder) in its cache folder, to ask you at the next start whether to report it. The note goes once it is reported or you choose not to; at most the five newest are kept, for 30 days.

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
| `gigabait.uk` (TensaCraft edition only) | The TensaCraft servers' builds on Home, and their pictures, from the addresses the server gives (the launcher asks whether a picture changed) | Nothing beyond the request |

When you start the game, Minecraft itself talks to Mojang and Microsoft under [their privacy statement](https://privacy.microsoft.com/privacystatement).

## The Windows registry

By default the launcher writes nothing to the Windows registry: the GPU mode is **Auto**, and Windows picks the graphics card for the game.

If you choose **Integrated** or **Discrete** (Settings → Java → "GPU mode for new builds", or a build's settings → Runtime), the launcher asks first, then writes that choice for the game's Java where Windows keeps per-program GPU choices: the same place Windows' own Settings → System → Display → Graphics writes to.

| | |
|---|---|
| Key | `HKEY_CURRENT_USER\Software\Microsoft\DirectX\UserGpuPreferences` |
| Value name | the full path of the build's Java (`javaw.exe`) |
| Value | `GpuPreference=1;` for integrated, `GpuPreference=2;` for discrete |

- Only the current user's part of the registry, no administrator rights. Nothing else is read or changed.
- Why: on a laptop with two graphics cards Windows may run the game on the slower built-in one; this is how to tell it otherwise.
- Some antivirus programs watch registry changes and may warn about this one. That is expected.
- To take it back, switch to **Auto**: at the next game start the launcher deletes the entries it wrote. You can also remove them in Windows Settings → Graphics.

The Windows installer adds the usual entries of an installed program (its line in the list of installed apps, for uninstalling it). On Linux and macOS the launcher keeps nothing like this: on Linux, **Discrete** only sets NVIDIA's PRIME variables for the game's process.

## Reports

Reports are about the launcher itself, so that its problems can be found and fixed. A report is sent only when you press a button, to `gigabait.uk`:

- "Report" on the notice of a failed operation (installing a version, a loader or a modpack, updating the launcher…), when the failure looks like the launcher's and not your network's, account's or computer's;
- "A problem with the launcher" in the Support window, where you describe what happened;
- the question after the launcher crashed, at its next start ("Don't send" deletes the crash's note);
- in the TensaCraft edition: the error window of a server build's install on Home.

A report holds:
- the error's text and code, or your description;
- the launcher's version, the system (OS) and the launcher's recent activity;
- for a crash of the launcher: its error's text and place in the launcher's code;
- the launcher's own log;
- your contact, only if you entered one.

A report never holds the game's logs, crash reports or files. When the game crashes, the launcher only opens those files for you, on your computer: a game's crash is most often its build's mods, not the launcher's.

Before a report is sent, the launcher replaces your home folder's path with `<USER_HOME>` and removes sign-in tokens from its log.

To have a report you sent deleted, ask on [Discord](https://discord.com/invite/mftAjQA4Pp).

## Deleting your data

- Delete an account on the launcher's Profiles page to delete its tokens.
- Delete the launcher's folders to delete everything it keeps.

## Contact

Questions about this page: [Discord](https://discord.com/invite/mftAjQA4Pp) or [GitHub Issues](https://github.com/TensaCraft/GileaLauncher/issues).

Last updated: 5 October 2026.
