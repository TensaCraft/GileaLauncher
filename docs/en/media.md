<p align="right"><b>English</b> · <a href="../uk/media.md">Українська</a></p>

# Pictures: the `media` branch

The repository's pictures are kept out of `main`, on the `media` branch. It shares no history with `main`, and every file on it has a stable address:

```
https://raw.githubusercontent.com/TensaCraft/GileaLauncher/media/<path>
```

## What is there

| Folder | What | Who loads it |
|---|---|---|
| `readme/<lang>/` | the README's hero and screenshots | `README.md` (`en_US`), `README.uk.md` (`uk_UA`) |
| `setup/<lang>/` | the first-run wizard's pictures (see [development.md](development.md#the-first-run-wizards-pictures)) | every released launcher, by address |
| `guides/` | the pictures of the Discord guides | Discord posts, by address |
| `releases/<version>/` | screenshots of a release | release announcements and the launcher's web pages, by address |

`<lang>` is the interface's language code (`en_US`, `uk_UA`). Every language has the same file names: `readme/en_US/builds.jpg` and `readme/uk_UA/builds.jpg` are the same screenshot in two languages.

## Rules

- **A link always names the branch,** `media/<path>`, never a commit SHA. Then replacing a file updates every place that shows it.
- **A name never changes.** Released launchers load `setup/`, the READMEs load `readme/`, Discord posts and web pages load `guides/` and `releases/`. Renaming or removing a file breaks them, and released launchers cannot be fixed.
- **A picture is replaced in place,** under the same name. Every page that shows it then shows the new one, with no change in `main`.
- **Only new commits** on the branch: no rebase, amend or force-push, so every checkout of it keeps working.
- **Pictures only.** The branch holds no scripts or other files.
- A commit message is one line that says what changed (`readme: new modpacks screenshot`).

## Replacing a picture

Work in a separate checkout of the branch, so `main` stays as it is:

```bash
git fetch origin media
git worktree add ../gilea-media media
cd ../gilea-media
# put the new file in place of the old one, under the same name
git add readme/uk_UA/builds.jpg
git commit -m "readme: builds screenshot with the new toolbar"
git push origin media
```

GitHub shows the new picture within a few minutes: it caches the files for a short while. Discord may keep showing the old one a little longer. When you are done: `git worktree remove ../gilea-media`.

## Adding

- **A screenshot for the README.** Take it in every language at the same size as the others (a 2400 px wide JPEG), put it at `readme/<lang>/<name>.jpg` under the same name in each language, and show it in both `README.md` and `README.uk.md`.
- **A screenshot for a web page or a post** (`guides/`, `releases/<version>/`). Take it at 1200 × 750 CSS px with twice the pixels and save a 2400 × 1500 JPEG, like the README's: a smaller one does not fill the page's column. Use the default look (accent and font) unless the picture is about another one.
- **The wizard's pictures** change only when Home's cards, the Play button or the sidebar change: released launchers show them.
- **A language.** `readme/<lang>/` gets the same names as the other languages. The wizard's pictures for it go to `setup/<lang>/`, where the launcher looks for them once it has that language. A README in that language links its own folder.
