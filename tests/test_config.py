"""Configuration loading, precedence, validation and persistence."""

from __future__ import annotations

import os
import stat
from pathlib import Path

import pytest
from pydantic import ValidationError

from gelbooru_downloader.config import (
    ENV_PREFIX,
    AppPaths,
    Config,
    config_template,
    load_config,
    save_config,
)
from gelbooru_downloader.exceptions import ConfigurationError


def write_toml(path: Path, body: str) -> Path:
    path.write_text(body, encoding="utf-8")
    return path


class TestDefaults:
    def test_sensible_defaults(self) -> None:
        config = Config()
        assert config.general.output_dir == Path("./downloads")
        assert config.general.concurrency == 5
        assert config.general.page_size == 100
        assert config.general.retries == 3
        assert config.general.timeout == 30.0
        assert config.general.skip_existing is True
        assert config.network.base_url == "https://gelbooru.com"
        assert config.network.user_agent.startswith("GelbooruCLI/")
        assert config.display.color is True
        assert config.display.show_banner is True

    def test_missing_config_file_is_not_an_error(self, tmp_path: Path) -> None:
        config, paths = load_config(tmp_path / "nope.toml", use_env=False)
        assert config.general.concurrency == 5
        assert paths.config_file == tmp_path / "nope.toml"

    def test_page_size_is_capped_at_the_api_limit(self) -> None:
        with pytest.raises(ValidationError):
            Config().general.__class__(page_size=5000)

    def test_concurrency_bounds(self) -> None:
        with pytest.raises(ValidationError):
            Config().general.__class__(concurrency=0)
        with pytest.raises(ValidationError):
            Config().general.__class__(concurrency=999)

    def test_base_url_must_be_http(self) -> None:
        with pytest.raises(ValidationError):
            Config().network.__class__(base_url="ftp://example.invalid")

    def test_filename_template_rejects_separators(self) -> None:
        with pytest.raises(ValidationError):
            Config().general.__class__(filename_template="../{id}")
        with pytest.raises(ValidationError):
            Config().general.__class__(filename_template="a/{id}")

    def test_user_agent_rejects_newlines(self) -> None:
        with pytest.raises(ValidationError):
            Config().network.__class__(user_agent="bad\nagent")


class TestLoading:
    def test_reads_values_from_file(self, tmp_path: Path) -> None:
        path = write_toml(
            tmp_path / "config.toml",
            """
[general]
concurrency = 9
output_dir = "pics"

[network]
base_url = "https://example.invalid"
user_id = "1234"
api_key = "abc"
""",
        )
        config, _paths = load_config(path, use_env=False)
        assert config.general.concurrency == 9
        assert config.general.output_dir == Path("pics")
        assert config.network.base_url == "https://example.invalid"
        assert config.network.has_credentials is True

    def test_invalid_toml_is_reported(self, tmp_path: Path) -> None:
        path = write_toml(tmp_path / "config.toml", "this is not = = toml")
        with pytest.raises(ConfigurationError, match="not valid TOML"):
            load_config(path, use_env=False)

    def test_unknown_section_is_reported(self, tmp_path: Path) -> None:
        path = write_toml(tmp_path / "config.toml", "[nonsense]\nx = 1\n")
        with pytest.raises(ConfigurationError, match="unknown sections"):
            load_config(path, use_env=False)

    def test_invalid_value_is_reported(self, tmp_path: Path) -> None:
        path = write_toml(tmp_path / "config.toml", '[general]\nconcurrency = "many"\n')
        with pytest.raises(ConfigurationError, match="invalid configuration"):
            load_config(path, use_env=False)

    def test_partial_file_keeps_defaults(self, tmp_path: Path) -> None:
        path = write_toml(tmp_path / "config.toml", "[general]\nconcurrency = 2\n")
        config, _ = load_config(path, use_env=False)
        assert config.general.retries == 3
        assert config.general.concurrency == 2


class TestPrecedence:
    def test_env_overrides_file(self, tmp_path: Path) -> None:
        path = write_toml(tmp_path / "config.toml", "[general]\nconcurrency = 2\n")
        env = {f"{ENV_PREFIX}CONCURRENCY": "11"}
        config, _ = load_config(path, environ=env)
        assert config.general.concurrency == 11

    def test_explicit_overrides_beat_env(self, tmp_path: Path) -> None:
        env = {f"{ENV_PREFIX}CONCURRENCY": "11"}
        config, _ = load_config(
            tmp_path / "config.toml",
            environ=env,
            overrides={"general.concurrency": 3},
        )
        assert config.general.concurrency == 3

    def test_env_can_be_disabled(self, tmp_path: Path) -> None:
        env = {f"{ENV_PREFIX}CONCURRENCY": "11"}
        config, _ = load_config(tmp_path / "config.toml", environ=env, use_env=False)
        assert config.general.concurrency == 5

    def test_env_covers_every_documented_variable(self, tmp_path: Path) -> None:
        env = {
            f"{ENV_PREFIX}OUTPUT_DIR": "/tmp/pics",
            f"{ENV_PREFIX}CONCURRENCY": "7",
            f"{ENV_PREFIX}PAGE_SIZE": "20",
            f"{ENV_PREFIX}RETRIES": "1",
            f"{ENV_PREFIX}TIMEOUT": "12.5",
            f"{ENV_PREFIX}SKIP_EXISTING": "false",
            f"{ENV_PREFIX}BASE_URL": "https://mirror.invalid",
            f"{ENV_PREFIX}API_KEY": "envkey",
            f"{ENV_PREFIX}USER_ID": "999",
            f"{ENV_PREFIX}AUTH_MODE": "basic",
            f"{ENV_PREFIX}COLOR": "no",
            f"{ENV_PREFIX}SHOW_BANNER": "0",
        }
        config, _ = load_config(tmp_path / "config.toml", environ=env)
        assert config.general.output_dir == Path("/tmp/pics")
        assert config.general.concurrency == 7
        assert config.general.page_size == 20
        assert config.general.retries == 1
        assert config.general.timeout == 12.5
        assert config.general.skip_existing is False
        assert config.network.base_url == "https://mirror.invalid"
        assert config.network.api_key == "envkey"
        assert config.network.user_id == "999"
        assert config.network.auth_mode == "basic"
        assert config.display.color is False
        assert config.display.show_banner is False

    def test_none_overrides_are_ignored(self, tmp_path: Path) -> None:
        config, _ = load_config(tmp_path / "config.toml", overrides={"general.concurrency": None})
        assert config.general.concurrency == 5


class TestPaths:
    def test_explicit_config_file_relocates_state(self, tmp_path: Path) -> None:
        _, paths = load_config(tmp_path / "nested" / "config.toml", use_env=False)
        assert paths.config_file == tmp_path / "nested" / "config.toml"
        assert paths.presets_file == tmp_path / "nested" / "presets.json"
        assert paths.database_file == tmp_path / "nested" / "history.db"

    def test_home_override(self, tmp_path: Path) -> None:
        _, paths = load_config(use_env=True, environ={f"{ENV_PREFIX}HOME": str(tmp_path)})
        assert paths.config_dir == (tmp_path / "gelbooru-downloader").resolve()

    def test_portable_mode_keeps_state_local(self, tmp_path: Path) -> None:
        _, paths = load_config(portable=True, environ={})
        assert ".gelbooru-downloader" in str(paths.config_dir)

    def test_platform_paths_are_absolute(self) -> None:
        paths = AppPaths.discover()
        assert paths.config_file.is_absolute()
        assert paths.database_file.is_absolute()
        assert paths.config_dir != paths.data_dir or os.name == "nt"

    def test_ensure_creates_private_directories(self, tmp_path: Path) -> None:
        paths = AppPaths.discover(home=tmp_path).ensure()
        assert paths.config_dir.is_dir()
        mode = stat.S_IMODE(paths.config_dir.stat().st_mode)
        assert mode == 0o700


class TestMutating:
    def test_set_by_dotted_key(self) -> None:
        config = Config()
        config.set("general.concurrency", "12")
        assert config.general.concurrency == 12

    def test_set_by_bare_name(self) -> None:
        config = Config()
        config.set("concurrency", "4")
        assert config.general.concurrency == 4

    def test_set_coerces_types(self) -> None:
        config = Config()
        config.set("general.skip_existing", "no")
        config.set("general.timeout", "45")
        config.set("general.output_dir", "~/pics")
        assert config.general.skip_existing is False
        assert config.general.timeout == 45.0
        assert str(config.general.output_dir).startswith(str(Path.home()))

    def test_unknown_key_raises(self) -> None:
        with pytest.raises(ConfigurationError, match="unknown configuration key"):
            Config().set("nonsense", "1")

    def test_unknown_section_raises(self) -> None:
        with pytest.raises(ConfigurationError, match="unknown configuration section"):
            Config().set("nonsense.key", "1")

    def test_invalid_value_raises_value_error(self) -> None:
        with pytest.raises(ValueError, match="invalid integer"):
            Config().set("general.concurrency", "many")

    def test_get_reads_values(self) -> None:
        config = Config()
        assert config.get("general.retries") == 3
        assert config.get("retries") == 3

    def test_iter_fields_covers_every_setting(self) -> None:
        keys = {dotted for dotted, *_rest in Config().iter_fields()}
        assert "general.concurrency" in keys
        assert "network.api_key" in keys
        assert "network.referer" in keys
        assert "display.color" in keys
        assert len(keys) == sum(
            len(model.model_fields) for model in Config().section_models().values()
        )


class TestSecrets:
    def test_redacted_hides_api_key(self) -> None:
        config = Config()
        config.network.api_key = "super-secret"
        redacted = config.redacted()
        assert redacted["network"]["api_key"] == "********"
        assert "super-secret" not in str(redacted)

    def test_redacted_handles_empty_key(self) -> None:
        assert Config().redacted()["network"]["api_key"] is None


class TestPersistence:
    def test_save_round_trip(self, tmp_path: Path) -> None:
        config = Config()
        config.general.concurrency = 13
        config.network.api_key = "key"
        target = save_config(config, tmp_path / "sub" / "config.toml")

        assert target.is_file()
        reloaded, _ = load_config(target, use_env=False)
        assert reloaded.general.concurrency == 13
        assert reloaded.network.api_key == "key"
        assert reloaded.general.page_size == 100

    def test_saved_file_is_private(self, tmp_path: Path) -> None:
        target = save_config(Config(), tmp_path / "config.toml")
        assert stat.S_IMODE(target.stat().st_mode) == 0o600

    def test_saved_file_parses_as_toml(self, tmp_path: Path) -> None:
        import tomllib

        target = save_config(Config(), tmp_path / "config.toml")
        data = tomllib.loads(target.read_text())
        assert set(data) == {"general", "network", "display"}

    def test_template_is_valid(self, tmp_path: Path) -> None:
        target = tmp_path / "config.toml"
        target.write_text(config_template(), encoding="utf-8")
        config, _ = load_config(target, use_env=False)
        assert config.general.concurrency == 5
