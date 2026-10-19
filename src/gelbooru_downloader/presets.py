"""Named tag presets ("collections").

Presets are stored as a small JSON document in the user configuration directory
next to ``config.toml``. Writes are atomic and the file is created ``0600``
because presets may contain private tag queries.
"""

from __future__ import annotations

import json
import re
from collections.abc import Iterable, Iterator
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

from .exceptions import ConfigurationError, StorageError
from .models import utc_now_iso
from .utils import atomic_write_text, parse_tag_expression

__all__ = ["Preset", "PresetStore"]

#: Preset names are used as folder names, so keep them boring.
_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 _.\-]{0,63}$")

_SCHEMA_VERSION = 1


@dataclass(frozen=True, slots=True)
class Preset:
    """A saved tag search."""

    name: str
    tags: str
    folder: str | None = None
    notes: str = ""
    created_at: str = field(default_factory=utc_now_iso)
    updated_at: str = field(default_factory=utc_now_iso)
    limit: int | None = None

    def __post_init__(self) -> None:
        if not _NAME_RE.match(self.name):
            msg = (
                f"invalid preset name {self.name!r}: use letters, digits, spaces, "
                "'.', '-' or '_' (max 64 characters)"
            )
            raise ValueError(msg)
        if not self.tags.strip():
            msg = f"preset {self.name!r} needs at least one tag"
            raise ValueError(msg)

    @property
    def search_tags(self) -> str:
        """Tags with exclusions preserved, ready for the API."""

        return parse_tag_expression(self.tags).search_tags

    def to_dict(self) -> dict[str, Any]:
        return {
            "tags": self.tags,
            "folder": self.folder,
            "notes": self.notes,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "limit": self.limit,
        }

    @classmethod
    def from_dict(cls, name: str, data: Any) -> Preset:
        if not isinstance(data, dict):
            msg = f"preset {name!r} is malformed (expected an object)"
            raise ConfigurationError(msg)
        tags = data.get("tags")
        if not isinstance(tags, str) or not tags.strip():
            msg = f"preset {name!r} has no tags"
            raise ConfigurationError(msg)
        folder = data.get("folder")
        notes = data.get("notes", "")
        limit = data.get("limit")
        return cls(
            name=name,
            tags=tags.strip(),
            folder=folder.strip() if isinstance(folder, str) and folder.strip() else None,
            notes=notes if isinstance(notes, str) else "",
            created_at=str(data.get("created_at") or utc_now_iso()),
            updated_at=str(data.get("updated_at") or utc_now_iso()),
            limit=int(limit) if isinstance(limit, int) and limit > 0 else None,
        )

    def with_tags(self, tags: str) -> Preset:
        return replace(self, tags=tags, updated_at=utc_now_iso())


class PresetStore:
    """CRUD access to the presets JSON document."""

    def __init__(self, path: Path | str) -> None:
        self.path = Path(path)

    # -- storage ----------------------------------------------------------- #
    def _load_raw(self) -> dict[str, Any]:
        if not self.path.exists():
            return {"version": _SCHEMA_VERSION, "presets": {}}
        try:
            with self.path.open(encoding="utf-8") as handle:
                data = json.load(handle)
        except json.JSONDecodeError as exc:
            msg = f"{self.path} is not valid JSON: {exc}"
            raise StorageError(msg) from exc
        except OSError as exc:
            msg = f"cannot read presets from {self.path}: {exc}"
            raise StorageError(msg) from exc
        if not isinstance(data, dict):
            msg = f"{self.path} has an unexpected structure"
            raise StorageError(msg)
        presets = data.get("presets", {})
        if not isinstance(presets, dict):
            msg = f"{self.path} has an unexpected 'presets' section"
            raise StorageError(msg)
        return {"version": data.get("version", _SCHEMA_VERSION), "presets": presets}

    def _save_raw(self, data: dict[str, Any]) -> None:
        payload = json.dumps(data, indent=2, ensure_ascii=False, sort_keys=True) + "\n"
        try:
            atomic_write_text(self.path, payload, mode=0o600)
        except OSError as exc:
            msg = f"cannot write presets to {self.path}: {exc}"
            raise StorageError(msg) from exc

    # -- queries ----------------------------------------------------------- #
    def __len__(self) -> int:
        return len(self._load_raw()["presets"])

    def __iter__(self) -> Iterator[Preset]:
        raw = self._load_raw()["presets"]
        for name in sorted(raw):
            try:
                yield Preset.from_dict(name, raw[name])
            except (ValueError, ConfigurationError):
                continue

    def names(self) -> list[str]:
        return sorted(self._load_raw()["presets"])

    def exists(self, name: str) -> bool:
        return name in self._load_raw()["presets"]

    def get(self, name: str) -> Preset | None:
        raw = self._load_raw()["presets"].get(name)
        if raw is None:
            return None
        return Preset.from_dict(name, raw)

    def get_or_raise(self, name: str) -> Preset:
        preset = self.get(name)
        if preset is None:
            known = ", ".join(self.names()) or "none"
            msg = f"unknown preset {name!r} (known presets: {known})"
            raise ConfigurationError(msg)
        return preset

    # -- mutations --------------------------------------------------------- #
    def add(
        self,
        name: str,
        tags: str | Iterable[str],
        *,
        folder: str | None = None,
        notes: str = "",
        limit: int | None = None,
        overwrite: bool = False,
    ) -> Preset:
        """Create or replace a preset."""

        if isinstance(tags, str):
            tag_string = " ".join(tags.split())
        else:
            tag_string = " ".join(" ".join(str(tag).split()) for tag in tags if str(tag).strip())
        try:
            preset = Preset(
                name=name.strip(),
                tags=tag_string,
                folder=folder.strip() if folder else None,
                notes=notes,
                limit=limit,
            )
        except ValueError as exc:
            raise ConfigurationError(str(exc)) from exc

        data = self._load_raw()
        existing = data["presets"].get(preset.name)
        if existing is not None and not overwrite:
            msg = f"preset {preset.name!r} already exists (use --force to replace it)"
            raise ConfigurationError(msg)
        data["version"] = _SCHEMA_VERSION
        data["presets"][preset.name] = preset.to_dict()
        self._save_raw(data)
        return preset

    def remove(self, name: str) -> bool:
        data = self._load_raw()
        if name not in data["presets"]:
            return False
        del data["presets"][name]
        self._save_raw(data)
        return True

    def rename(self, old: str, new: str) -> Preset:
        data = self._load_raw()
        if old not in data["presets"]:
            msg = f"unknown preset {old!r}"
            raise ConfigurationError(msg)
        if new in data["presets"]:
            msg = f"preset {new!r} already exists"
            raise ConfigurationError(msg)
        try:
            preset = Preset.from_dict(new, data["presets"][old])
        except ValueError as exc:
            raise ConfigurationError(str(exc)) from exc
        del data["presets"][old]
        data["presets"][preset.name] = preset.to_dict()
        self._save_raw(data)
        return preset

    def clear(self) -> int:
        data = self._load_raw()
        count = len(data["presets"])
        data["presets"] = {}
        self._save_raw(data)
        return count
