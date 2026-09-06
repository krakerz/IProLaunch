# helper-script/

Per-desktop-environment file-manager context-menu integration ("Add to
IProLaunch Library" on right-click), embedded into the `iprolaunch` binary
at compile time (`include_str!` in `src/context_menu.rs`) and installed via:

```sh
iprolaunch context-menu install        # installs every DE below
iprolaunch context-menu install kde    # just one
iprolaunch context-menu uninstall [de]
```

Each file below is a template — `{bin}` is replaced with this exact running
binary's path, `{mimetypes}` (KDE only) with the same mimetype list
`integrate.rs` uses for `.exe`/`.bat`/`.cmd`/`.msi`.

- `kde/iprolaunch-add.desktop` — a KIO service menu (Dolphin's right-click
  integration mechanism). Installed to both
  `~/.local/share/kio/servicemenus/` (Plasma 6) and
  `~/.local/share/kservices5/ServiceMenus/` (Plasma 5) since either might be
  what a given system actually reads — the other just sits unused.
- `gnome/Add to IProLaunch Library` — a "Nautilus Scripts" shell script
  (the filename *is* the menu label). Installed to Nautilus's, Nemo's, and
  Caja's own scripts folders — all three file managers support this same
  drop-a-script-in-a-folder convention (Nemo even understands Nautilus's own
  env var), so one script covers GNOME, Cinnamon, and MATE.
- `xfce/uca-action.xml` — a single `<action>` block merged into Thunar's
  `~/.config/Thunar/uca.xml` (Thunar has no drop-in-a-folder mechanism —
  every custom action lives in that one file, so this is merged in/out by
  `context_menu.rs`'s own text transform rather than just copied, identified
  by a stable `<unique-id>` so re-running `install` updates rather than
  duplicates it, and `uninstall` only removes this one entry).

Not covered: anything that isn't KDE/GNOME/Cinnamon/MATE/XFCE (e.g. LXQt,
budgie's own file manager if not Nautilus-based) — genuinely different
mechanisms that would need their own research; `context-menu install`
simply doesn't touch those.
