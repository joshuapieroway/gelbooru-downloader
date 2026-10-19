"""Named preset storage."""

from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from gelbooru_downloader.exceptions import ConfigurationError, StorageError
from gelbooru_downloader.presets import Preset, PresetStore


@pytest.fixture
def store(tmp_path: Path) -> PresetStore:
    return PresetStore(tmp_path / "presets.json")


class TestPreset:
    def test_requires_tags(self) -> None:
        with pytest.raises(ValueError, match="at least one tag"):
            Preset(name="x", tags="   ")

    @pytest.mark.parametrize("name", ["", "-bad", "a" * 100, "bad/name", "bad\\name"])
    def test_invalid_names_rejected(self, name: str) -> None:
        with pytest.raises(ValueError, match="invalid preset name"):
            Preset(name=name, tags="solo")

    @pytest.mark.parametrize("name", ["anime", "my-set", "set_2", "Set Of Tags", "v1.0"])
    def test_valid_names_accepted(self, name: str) -> None:
        assert Preset(name=name, tags="solo").name == name

    def test_search_tags_keeps_exclusions(self) -> None:
        assert Preset(name="a", tags="solo -nude").search_tags == "solo -nude"

    def test_from_dict_validates(self) -> None:
        assert Preset.from_dict("a", {"tags": "solo"}).tags == "solo"
        with pytest.raises(ConfigurationError):
            Preset.from_dict("a", {"tags": ""})
        with pytest.raises(ConfigurationError):
            Preset.from_dict("a", "not a dict")


class TestStore:
    def test_empty_store(self, store: PresetStore) -> None:
        assert len(store) == 0
        assert store.names() == []
        assert store.get("nope") is None
        assert list(store) == []

    def test_add_and_get(self, store: PresetStore) -> None:
        store.add("anime", "blue_hair solo", folder="anime-folder", limit=25, notes="favourite")
        preset = store.get("anime")
        assert preset is not None
        assert preset.tags == "blue_hair solo"
        assert preset.folder == "anime-folder"
        assert preset.limit == 25
        assert preset.notes == "favourite"
        assert preset.created_at

    def test_add_accepts_a_list(self, store: PresetStore) -> None:
        store.add("multi", ["solo", "blue_hair"])
        assert store.get("multi").tags == "solo blue_hair"  # type: ignore[union-attr]

    def test_duplicate_requires_force(self, store: PresetStore) -> None:
        store.add("anime", "solo")
        with pytest.raises(ConfigurationError, match="already exists"):
            store.add("anime", "other")
        store.add("anime", "other", overwrite=True)
        assert store.get("anime").tags == "other"  # type: ignore[union-attr]

    def test_invalid_name_is_a_configuration_error(self, store: PresetStore) -> None:
        with pytest.raises(ConfigurationError, match="invalid preset name"):
            store.add("bad/name", "solo")

    def test_remove(self, store: PresetStore) -> None:
        store.add("a", "solo")
        assert store.remove("a") is True
        assert store.remove("a") is False
        assert store.names() == []

    def test_rename(self, store: PresetStore) -> None:
        store.add("old", "solo", folder="f")
        renamed = store.rename("old", "new")
        assert renamed.name == "new"
        assert store.get("old") is None
        assert store.get("new").folder == "f"  # type: ignore[union-attr]
        with pytest.raises(ConfigurationError, match="unknown preset"):
            store.rename("missing", "x")
        with pytest.raises(ConfigurationError, match="already exists"):
            store.rename("new", "new")

    def test_listing_is_sorted(self, store: PresetStore) -> None:
        store.add("zeta", "solo")
        store.add("alpha", "solo")
        assert store.names() == ["alpha", "zeta"]
        assert [preset.name for preset in store] == ["alpha", "zeta"]

    def test_get_or_raise_lists_known_names(self, store: PresetStore) -> None:
        store.add("alpha", "solo")
        with pytest.raises(ConfigurationError, match="known presets: alpha"):
            store.get_or_raise("beta")

    def test_clear(self, store: PresetStore) -> None:
        store.add("a", "solo")
        store.add("b", "solo")
        assert store.clear() == 2
        assert store.names() == []

    def test_file_is_private_and_valid_json(self, store: PresetStore) -> None:
        store.add("anime", "solo")
        mode = stat.S_IMODE(store.path.stat().st_mode)
        assert mode == 0o600
        data = json.loads(store.path.read_text())
        assert data["presets"]["anime"]["tags"] == "solo"

    def test_corrupt_file_raises_storage_error(self, store: PresetStore) -> None:
        store.path.write_text("{not json")
        with pytest.raises(StorageError, match="not valid JSON"):
            store.names()

    def test_unexpected_structure_raises(self, store: PresetStore) -> None:
        store.path.write_text('{"presets": []}')
        with pytest.raises(StorageError, match="unexpected"):
            store.names()

    def test_broken_records_are_skipped_in_listing(self, store: PresetStore) -> None:
        store.path.write_text(
            json.dumps({"presets": {"good": {"tags": "solo"}, "bad": {"tags": ""}}})
        )
        assert [preset.name for preset in store] == ["good"]

    def test_missing_directory_is_created(self, tmp_path: Path) -> None:
        store = PresetStore(tmp_path / "deep" / "nested" / "presets.json")
        store.add("a", "solo")
        assert store.path.is_file()

    def test_update_changes_tags(self, store: PresetStore) -> None:
        preset = store.add("a", "solo")
        updated = preset.with_tags("dual tails")
        assert updated.tags == "dual tails"
        assert updated.updated_at >= preset.updated_at
