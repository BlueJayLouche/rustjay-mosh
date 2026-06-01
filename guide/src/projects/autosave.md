# Autosave & Recovery

Datamoshing is experimental work — you try things, and sometimes a decode does something unexpected. Autosave makes sure a crash never costs you the session.

## How autosave works

While you have **unsaved changes** and the app is **idle** (not mid-render, mid-bake, or mid-save), rustjay-mosh periodically writes a full recovery bundle in the background — about every two minutes. It's a complete [`.rjmosh` bundle](README.md), embedded media and all, written to a fixed location in your user config directory:

```
<config>/rustjay-mosh/autosave/recovery.rjmosh
```

The status line briefly notes *"Autosaved recovery snapshot."* when it happens. Autosave only runs when there's something new to save and the app isn't busy, so it never interrupts a render or a bake.

## Recovery on next launch

If the app finds a recovery bundle when it starts, it shows a banner:

> ⚠ An autosaved recovery snapshot was found. **[Recover] [Dismiss]**

- **Recover** loads the snapshot, restoring your timeline and all media exactly as of the last autosave.
- **Dismiss** hides the banner and leaves the snapshot in place.

## Autosave hygiene

A **clean save** (`Ctrl+S` to a real location) deletes the recovery snapshot — once your work is safely saved, the recovery copy is redundant and won't be offered again. Likewise, opening a project clears the banner.

> After recovering, the project's path points at the recovery bundle. Use **Save As…** to write it to a permanent location of your choosing.

## Where things live

| What | Location |
|---|---|
| Autosave recovery | `<config>/rustjay-mosh/autosave/recovery.rjmosh` |
| Recent projects list | `<config>/rustjay-mosh/recent.json` |

`<config>` is your platform's standard config directory (e.g. `~/Library/Application Support` on macOS, `~/.config` on Linux, `%APPDATA%` on Windows).
