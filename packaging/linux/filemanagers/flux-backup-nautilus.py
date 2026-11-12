# Flux Backup in Nautilus's right-click menu, under one "Flux Backup" submenu:
# "Previous versions…" on a file or folder,
# "Restore deleted files here…" on a folder or a folder's background, and "Leave out of
# backups…" (on what a backup takes) or "Back up this folder…" (on a folder none takes).
# Each starts the Flux Backup app (or hands the request to the one running), which finds
# the backups concerned and asks before changing anything. Needs nautilus-python (Fedora:
# nautilus-python; Debian/Ubuntu: python3-nautilus). Installed to
# /usr/share/nautilus-python/extensions/.

import os
import subprocess

try:
    import tomllib
except ImportError:  # Python before 3.11: both items are offered, the app sorts it out
    tomllib = None

import gi

for _version in ("4.1", "4.0", "3.0"):
    try:
        gi.require_version("Nautilus", _version)
        break
    except ValueError:
        continue

from gi.repository import GObject, Nautilus  # noqa: E402

APP = "flux-backup"


def _path(item):
    """The local path of a Nautilus file, or None (trash, network places…)."""
    if item.get_uri_scheme() != "file":
        return None
    location = item.get_location()
    return location.get_path() if location is not None else None


def _run(_menu_item, flag, path):
    subprocess.Popen(
        [APP, flag, path],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )


def _item(name, label, tip, flag, path):
    item = Nautilus.MenuItem(name=f"FluxBackup::{name}", label=label, tip=tip)
    item.connect("activate", _run, flag, path)
    return item


def _versions(path):
    return _item(
        "Versions",
        "Previous versions…",
        "Earlier versions of this in your Flux backups",
        "--previous-versions",
        path,
    )


def _leave_out(path):
    return _item(
        "LeaveOut",
        "Leave out of backups…",
        "Stop your Flux backups copying this, after asking which",
        "--leave-out",
        path,
    )


def _back_up(path):
    return _item(
        "BackUp",
        "Back up this folder…",
        "Add this folder to a Flux backup, or set up a new one",
        "--back-up",
        path,
    )


_PROFILES = os.path.join(
    os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"),
    "flux-backup",
    "profiles",
)
_cache = (None, None)


def _folders():
    """The folders your backups take (real paths), or None when that can't be told.
    Read again only when a backup's settings change (the folder's time changes)."""
    global _cache
    if tomllib is None:
        return None
    try:
        stamp = os.stat(_PROFILES).st_mtime_ns
    except FileNotFoundError:
        return []
    except OSError:
        return None
    if _cache[0] == stamp:
        return _cache[1]
    folders = []
    for name in os.listdir(_PROFILES):
        if not name.endswith(".toml"):
            continue
        try:
            with open(os.path.join(_PROFILES, name), "rb") as f:
                data = tomllib.load(f)
        except (OSError, ValueError):
            continue
        for p in data.get("sources", {}).get("paths", []):
            folders.append(os.path.realpath(p))
    _cache = (stamp, folders)
    return folders


def _taken(path):
    """True: a backup takes it (its exclusions aside); False: none does; None: unknown."""
    folders = _folders()
    if folders is None:
        return None
    real = os.path.realpath(path)
    return any(real == f or real.startswith(f.rstrip("/") + "/") for f in folders)


def _which(path, is_dir):
    """Leave out what a backup takes; back up a folder none takes."""
    taken = _taken(path)
    items = []
    if taken is not False:
        items.append(_leave_out(path))
    if is_dir and taken is not True:
        items.append(_back_up(path))
    return items


def _deleted(path):
    return _item(
        "Deleted",
        "Restore deleted files here…",
        "Files your Flux backups have from this folder that aren't here any more",
        "--restore-deleted",
        path,
    )


def _submenu(items, where):
    """Everything under one "Flux Backup" entry, so the menu stays short. `where`:
    `File` or `Background` (each entry's name is its own)."""
    top = Nautilus.MenuItem(
        name=f"FluxBackup::{where}Menu",
        label="Flux Backup",
        tip="Your Flux backups of this",
    )
    menu = Nautilus.Menu()
    for item in items:
        menu.append_item(item)
    top.set_submenu(menu)
    return [top]


class FluxBackupMenu(GObject.GObject, Nautilus.MenuProvider):
    # Nautilus 43 and later call these with the files only; older ones pass the window
    # first: take the last argument either way.
    def get_file_items(self, *args):
        files = args[-1]
        if len(files) != 1:
            return []
        path = _path(files[0])
        if not path:
            return []
        is_dir = files[0].is_directory()
        items = [_versions(path)]
        if is_dir:
            items.append(_deleted(path))
        return _submenu(items + _which(path, is_dir), "File")

    def get_background_items(self, *args):
        path = _path(args[-1])
        if not path:
            return []
        return _submenu([_deleted(path), _versions(path)] + _which(path, True), "Background")
