"""End-to-end CLI tests.

HTTP is mocked at the ``build_client``/``Downloader`` boundary, so these tests
exercise argument parsing, rendering, wiring and exit codes without touching the
network.
"""

from __future__ import annotations

import json
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import httpx
import pytest
from typer.testing import CliRunner

from gelbooru_downloader import __version__
from gelbooru_downloader.app import build_client, set_context
from gelbooru_downloader.cli import app
from gelbooru_downloader.downloader import Downloader

from .conftest import json_page, post_payload, xml_page

PNG = b"\x89PNG\r\n\x1a\n" + b"data" * 40

API_REQUESTS: list[httpx.URL] = []
IMAGE_REQUESTS: list[httpx.URL] = []


def api_handler(request: httpx.Request) -> httpx.Response:
    API_REQUESTS.append(request.url)
    params = dict(request.url.params)
    if params.get("page") != "dapi":
        return httpx.Response(404)
    if "status=fail" in params.get("tags", ""):
        return httpx.Response(500)
    tags = params.get("tags", "")
    limit = min(int(params.get("limit", "100")), 100)
    pid = int(params.get("pid", "0"))  # Gelbooru's pid is a zero-based page index
    if tags.startswith("id:"):
        wanted = [int(part.split(":")[1]) for part in tags.split()]
        records = [post_payload(id=str(post_id)) for post_id in wanted]
        return httpx.Response(200, text=json_page(records))
    # Vary the ids per query so different searches yield different posts.
    offset_by_tags = sum(ord(char) for char in tags) % 7 * 100
    start = pid * limit
    # Echo the requested rating back, like Gelbooru does, so client-side
    # filtering is exercised realistically.
    rating = next(
        (part.split(":", 1)[1] for part in tags.split() if part.startswith("rating:")),
        "general",
    )
    records = [
        post_payload(id=str(1000 + offset_by_tags + start + i), rating=rating) for i in range(limit)
    ]
    return httpx.Response(200, text=json_page(records, count=1000, offset=start))


def image_handler(request: httpx.Request) -> httpx.Response:
    IMAGE_REQUESTS.append(request.url)
    return httpx.Response(200, content=PNG, headers={"content-type": "image/png"})


@pytest.fixture(autouse=True)
def mock_transport(monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    """Route every API call and image download through MockTransport."""

    API_REQUESTS.clear()
    IMAGE_REQUESTS.clear()

    def fake_build_client(config: Any, **kwargs: Any) -> Any:
        kwargs.pop("transport", None)
        client = build_client(config, transport=httpx.MockTransport(api_handler), **kwargs)
        return client

    original_downloader_init = Downloader.__init__

    def patched_init(self: Downloader, options: Any, **kwargs: Any) -> None:
        kwargs["transport"] = httpx.MockTransport(image_handler)
        original_downloader_init(self, options, **kwargs)

    monkeypatch.setattr("gelbooru_downloader.app.build_client", fake_build_client)
    monkeypatch.setattr(Downloader, "__init__", patched_init)
    yield
    set_context(None)


@pytest.fixture
def runner(monkeypatch: pytest.MonkeyPatch) -> CliRunner:
    # Rich sizes itself from COLUMNS when stdout is not a terminal.
    monkeypatch.setenv("COLUMNS", "160")
    monkeypatch.setenv("TERM", "dumb")
    return CliRunner()


@pytest.fixture
def cli(runner: CliRunner, tmp_path: Path) -> Callable[..., Any]:
    """Invoke the CLI with an isolated config file."""

    config = tmp_path / "config.toml"

    def invoke(*args: str, expect_success: bool = True) -> Any:
        result = runner.invoke(app, ["--config", str(config), "--no-color", *args])
        if expect_success and result.exit_code != 0:  # pragma: no cover - debugging aid
            raise AssertionError(
                f"command {args} failed with {result.exit_code}\n{result.output}\n{result.exception!r}"
            )
        return result

    invoke.config = config  # type: ignore[attr-defined]
    invoke.tmp_path = tmp_path  # type: ignore[attr-defined]
    return invoke


# --------------------------------------------------------------------------- #
# Help and version
# --------------------------------------------------------------------------- #
class TestHelp:
    def test_root_help_lists_commands(self, runner: CliRunner) -> None:
        result = runner.invoke(app, ["--help"])
        assert result.exit_code == 0
        for command in ("search", "download", "post", "presets", "config", "batch", "watch"):
            assert command in result.output

    @pytest.mark.parametrize(
        "command",
        [
            "search",
            "download",
            "batch",
            "watch",
            "version",
            "interactive",
            "post info",
            "post download",
            "presets list",
            "presets add",
            "presets download",
            "config show",
            "config set",
            "config history",
        ],
    )
    def test_subcommand_help(self, runner: CliRunner, command: str) -> None:
        result = runner.invoke(app, [*command.split(), "--help"])
        assert result.exit_code == 0
        assert "Usage" in result.output.replace("usage", "Usage")

    def test_no_arguments_shows_help(self, runner: CliRunner) -> None:
        result = runner.invoke(app, [])
        assert result.exit_code != 0
        assert "Usage" in result.output.replace("usage", "Usage")

    def test_version_command(self, cli: Callable[..., Any]) -> None:
        result = cli("version")
        assert __version__ in result.output
        assert "config" in result.output


# --------------------------------------------------------------------------- #
# Search
# --------------------------------------------------------------------------- #
class TestSearch:
    def test_search_renders_table(self, cli: Callable[..., Any]) -> None:
        result = cli("search", "blue_hair", "--limit", "3")
        first_id = str(1000 + sum(ord(char) for char in "blue_hair") % 7 * 100)
        assert first_id in result.output
        assert "Tags" in result.output
        assert "Dimensions" in result.output
        assert "matched" in result.output

    def test_search_passes_tags_and_limit(self, cli: Callable[..., Any]) -> None:
        cli("search", "blue_hair solo", "--limit", "2", "--page-size", "2")
        params = dict(API_REQUESTS[0].params)
        assert params["tags"] == "blue_hair solo"
        assert params["limit"] == "2"
        assert params["pid"] == "0"  # first page is pid 0

    def test_search_page_option(self, cli: Callable[..., Any]) -> None:
        cli("search", "solo", "--page", "3", "--limit", "1")
        assert dict(API_REQUESTS[0].params)["pid"] == "2"

    def test_search_rating_metatag(self, cli: Callable[..., Any]) -> None:
        cli("search", "solo", "--rating", "safe", "--limit", "1")
        assert "rating:safe" in dict(API_REQUESTS[0].params)["tags"]

    def test_search_excludes_tags(self, cli: Callable[..., Any]) -> None:
        cli("search", "solo", "--exclude", "nude,blurry", "--limit", "1")
        tags = dict(API_REQUESTS[0].params)["tags"]
        assert "-nude" in tags
        assert "-blurry" in tags

    def test_search_no_results_message(self, cli: Callable[..., Any], monkeypatch: Any) -> None:
        monkeypatch.setattr(
            "gelbooru_downloader.app.build_client",
            lambda config, **kwargs: build_client(
                config,
                transport=httpx.MockTransport(lambda r: httpx.Response(200, text=json_page([]))),
            ),
        )
        result = cli("search", "nothing_matches_this", "--limit", "5")
        assert result.exit_code == 0
        assert "No posts matched" in result.output

    def test_export_json(self, cli: Callable[..., Any]) -> None:
        target = cli.tmp_path / "out.json"
        cli("search", "solo", "--limit", "2", "--format", "json", "--output", str(target))
        data = json.loads(target.read_text())
        assert data["count"] == 2
        assert "file_url" in data["posts"][0]

    def test_export_csv(self, cli: Callable[..., Any]) -> None:
        target = cli.tmp_path / "out.csv"
        cli("search", "solo", "--limit", "2", "--format", "csv", "--output", str(target))
        lines = target.read_text().strip().splitlines()
        assert lines[0].startswith("id,rating,width")
        assert len(lines) == 3

    def test_export_unknown_format(self, cli: Callable[..., Any]) -> None:
        result = cli("search", "solo", "--format", "xml", expect_success=False)
        assert result.exit_code == 2
        assert "unknown --format" in result.output

    def test_rating_or_fans_out_into_two_queries(self, cli: Callable[..., Any]) -> None:
        cli("search", "solo", "--rating", "questionable,explicit", "--limit", "3")
        tag_queries = [
            params["tags"]
            for params in (dict(url.params) for url in API_REQUESTS)
            if params["tags"]
        ]
        assert "solo rating:questionable" in tag_queries
        assert "solo rating:explicit" in tag_queries
        assert "solo rating:questionable,explicit" not in tag_queries

    def test_rating_or_merges_results_without_duplicates(
        self, cli: Callable[..., Any], monkeypatch: Any
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            params = dict(request.url.params)
            posts = (
                [post_payload(id="500", rating="questionable")]
                if params["tags"].endswith("rating:questionable")
                else [post_payload(id="600", rating="explicit")]
            )
            return httpx.Response(200, text=json_page(posts, count=2))

        monkeypatch.setattr(
            "gelbooru_downloader.app.build_client",
            lambda config, **kwargs: build_client(config, transport=httpx.MockTransport(handler)),
        )
        result = cli("search", "solo", "--rating", "questionable,explicit", "--limit", "10")
        assert "500" in result.output
        assert "600" in result.output
        assert "2 fetched" in result.output

    def test_invalid_limit_is_a_usage_error(self, cli: Callable[..., Any]) -> None:
        result = cli("search", "solo", "--limit", "-3", expect_success=False)
        assert result.exit_code == 2

    def test_invalid_size_is_a_usage_error(self, cli: Callable[..., Any]) -> None:
        result = cli("search", "solo", "--min-size", "huge", expect_success=False)
        assert result.exit_code == 2

    def test_missing_tags_is_a_usage_error(self, cli: Callable[..., Any]) -> None:
        result = cli("search", expect_success=False)
        assert result.exit_code == 2

    def test_server_error_exits_non_zero(self, cli: Callable[..., Any]) -> None:
        result = cli("search", "status=fail", expect_success=False)
        assert result.exit_code == 1
        assert "✗" in result.output or "Error" in result.output

    def test_xml_responses_are_understood(self, cli: Callable[..., Any], monkeypatch: Any) -> None:
        monkeypatch.setattr(
            "gelbooru_downloader.app.build_client",
            lambda config, **kwargs: build_client(
                config,
                transport=httpx.MockTransport(
                    lambda r: httpx.Response(
                        200,
                        content=xml_page([post_payload(id="4242")]).encode(),
                        headers={"content-type": "text/xml"},
                    )
                ),
            ),
        )
        result = cli("search", "solo", "--limit", "1")
        assert "4242" in result.output


# --------------------------------------------------------------------------- #
# Download
# --------------------------------------------------------------------------- #
class TestDownload:
    def test_downloads_files_and_reports(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        result = cli("download", "blue_hair", "--limit", "3", "-o", str(out), "--no-progress")
        assert "Downloaded" in result.output
        assert len(list(out.glob("*.png"))) == 3

    def test_history_records_downloads(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "blue_hair", "--limit", "2", "-o", str(out), "--no-progress")
        result = cli("config", "history", "--limit", "5")
        assert "completed" in result.output
        assert "1000.png" in result.output or ".png" in result.output

    def test_second_run_skips_existing(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "blue_hair", "--limit", "2", "-o", str(out), "--no-progress")
        images_before = len(IMAGE_REQUESTS)
        result = cli("download", "blue_hair", "--limit", "2", "-o", str(out), "--no-progress")
        assert "Skipped" in result.output
        assert len(IMAGE_REQUESTS) == images_before

    def test_overwrite_refetches(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "blue_hair", "--limit", "1", "-o", str(out), "--no-progress")
        before = len(IMAGE_REQUESTS)
        cli("download", "blue_hair", "--limit", "1", "-o", str(out), "--overwrite", "--no-progress")
        assert len(IMAGE_REQUESTS) == before + 1

    def test_dry_run_downloads_nothing(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        result = cli("download", "blue_hair", "--limit", "2", "-o", str(out), "--dry-run")
        assert "would be downloaded" in result.output
        assert not out.exists()
        assert IMAGE_REQUESTS == []

    def test_collection_option(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "solo", "--limit", "1", "-o", str(out), "-C", "My Set", "--no-progress")
        assert (out / "My Set").is_dir()

    def test_folder_by_tags(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "solo", "--limit", "1", "-o", str(out), "--folder-by-tags", "--no-progress")
        assert (out / "blue_hair_solo").is_dir()

    def test_archive_writes_sidecars(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "solo", "--limit", "1", "-o", str(out), "--archive", "--no-progress")
        sidecars = list(out.glob("*.png.json"))
        assert len(sidecars) == 1
        assert json.loads(sidecars[0].read_text())["file_url"]

    def test_filename_template(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli(
            "download",
            "solo",
            "--limit",
            "1",
            "-o",
            str(out),
            "-F",
            "{id}-{rating}",
            "--no-progress",
        )
        # The mock echoes the rating metatag, so the default rating is "general".
        assert list(out.glob("*-general.png"))

    def test_min_size_filters_small_posts_out(self, cli: Callable[..., Any]) -> None:
        # Mocked posts declare 200 KiB, so a 10 MB minimum removes them before
        # any request is made — that is the point of filtering early.
        out = cli.tmp_path / "downloads"
        result = cli(
            "download",
            "solo",
            "--limit",
            "2",
            "-o",
            str(out),
            "--min-size",
            "10MB",
            "--no-progress",
        )
        assert "filtered out" in result.output
        assert not list(out.glob("*.png"))
        assert IMAGE_REQUESTS == []

    def test_min_size_skips_during_download_when_metadata_is_missing(
        self, cli: Callable[..., Any], monkeypatch: Any
    ) -> None:
        monkeypatch.setattr(
            "gelbooru_downloader.app.build_client",
            lambda config, **kwargs: build_client(
                config,
                transport=httpx.MockTransport(
                    lambda r: httpx.Response(
                        200,
                        text=json_page(
                            [post_payload(id=str(2000 + i), file_size="") for i in range(2)],
                            count=2,
                        ),
                    )
                ),
            ),
        )
        out = cli.tmp_path / "downloads"
        result = cli(
            "download",
            "solo",
            "--limit",
            "2",
            "-o",
            str(out),
            "--min-size",
            "10MB",
            "--no-progress",
        )
        assert "Skipped" in result.output
        assert not list(out.glob("*.png"))

    def test_no_duplicates(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        result = cli(
            "download", "solo", "--limit", "3", "-o", str(out), "--no-duplicates", "--no-progress"
        )
        assert "Duplicates" in result.output

    def test_concurrency_option_is_accepted(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        result = cli("download", "solo", "--limit", "2", "-o", str(out), "-c", "8", "--no-progress")
        assert "Downloaded" in result.output

    def test_no_matches_is_not_an_error(self, cli: Callable[..., Any], monkeypatch: Any) -> None:
        monkeypatch.setattr(
            "gelbooru_downloader.app.build_client",
            lambda config, **kwargs: build_client(
                config,
                transport=httpx.MockTransport(lambda r: httpx.Response(200, text=json_page([]))),
            ),
        )
        result = cli("download", "nothing", "-o", str(cli.tmp_path / "d"))
        assert result.exit_code == 0
        assert "No posts to download" in result.output

    def test_rating_filter_reaches_the_api(self, cli: Callable[..., Any]) -> None:
        # Regression: --rating was silently dropped in the download command, so
        # the filter never reached the API and every post was fetched.
        cli("download", "solo", "--rating", "safe", "--limit", "2", "--no-progress")
        assert "rating:safe" in dict(API_REQUESTS[0].params)["tags"]

    def test_rating_or_reaches_the_api(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "rating-or"
        cli(
            "download",
            "solo",
            "--rating",
            "questionable,explicit",
            "--limit",
            "2",
            "-o",
            str(out),
            "--no-progress",
        )
        queries = {dict(url.params)["tags"] for url in API_REQUESTS}
        assert "solo rating:questionable" in queries
        assert "solo rating:explicit" in queries
        assert len(list(out.glob("*.png"))) == 2

    def test_ext_filter_reaches_the_client(self, cli: Callable[..., Any]) -> None:
        # Same class of bug: --ext was dropped on the way into DownloadRequest.
        out = cli.tmp_path / "ext-out"
        cli("download", "solo", "--ext", "png", "--limit", "2", "-o", str(out), "--no-progress")
        assert len(list(out.glob("*.png"))) == 2

    def test_unknown_option_is_not_silently_dropped(self) -> None:
        from gelbooru_downloader.commands.download import _request_from_options

        with pytest.raises(ValueError, match="unknown download option"):
            _request_from_options(rating=["safe"])  # typo'd field name


# --------------------------------------------------------------------------- #
# post subcommands
# --------------------------------------------------------------------------- #
class TestPostCommands:
    def test_post_info(self, cli: Callable[..., Any]) -> None:
        result = cli("post", "info", "1000")
        assert "1000" in result.output
        assert "File URL" in result.output
        assert "Tags" in result.output

    def test_post_info_json(self, cli: Callable[..., Any]) -> None:
        result = cli("post", "info", "1000", "--json")
        assert json.loads(result.output)["id"] == 1000

    def test_post_info_missing(self, cli: Callable[..., Any], monkeypatch: Any) -> None:
        monkeypatch.setattr(
            "gelbooru_downloader.app.build_client",
            lambda config, **kwargs: build_client(
                config,
                transport=httpx.MockTransport(lambda r: httpx.Response(200, text=json_page([]))),
            ),
        )
        result = cli("post", "info", "999999", expect_success=False)
        assert result.exit_code == 1
        assert "not found" in result.output

    def test_post_download(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "posts"
        result = cli("post", "download", "1000", "1001", "-o", str(out), "--no-progress")
        assert "Downloaded" in result.output
        assert sorted(p.name for p in out.glob("*.png")) == ["1000.png", "1001.png"]

    def test_post_download_requires_ids(self, cli: Callable[..., Any]) -> None:
        result = cli("post", "download", expect_success=False)
        assert result.exit_code == 2

    def test_post_download_rejects_bad_id(self, cli: Callable[..., Any]) -> None:
        result = cli("post", "download", "not-a-number", expect_success=False)
        assert result.exit_code == 2


# --------------------------------------------------------------------------- #
# Presets
# --------------------------------------------------------------------------- #
class TestPresets:
    def test_add_list_remove(self, cli: Callable[..., Any]) -> None:
        cli("presets", "add", "anime", "blue_hair solo", "--limit", "10")
        listing = cli("presets", "list")
        assert "anime" in listing.output
        assert "blue_hair solo" in listing.output

        cli("presets", "remove", "anime")
        listing = cli("presets", "list")
        assert "No presets saved yet" in listing.output

    def test_remove_missing_exits_one(self, cli: Callable[..., Any]) -> None:
        result = cli("presets", "remove", "ghost", expect_success=False)
        assert result.exit_code == 1

    def test_duplicate_preset_is_rejected(self, cli: Callable[..., Any]) -> None:
        cli("presets", "add", "anime", "solo")
        result = cli("presets", "add", "anime", "other", expect_success=False)
        assert result.exit_code == 1
        assert "already exists" in result.output

    def test_force_overwrites(self, cli: Callable[..., Any]) -> None:
        cli("presets", "add", "anime", "solo")
        cli("presets", "add", "anime", "other", "--force")
        assert "other" in cli("presets", "show", "anime").output

    def test_show_unknown_preset(self, cli: Callable[..., Any]) -> None:
        result = cli("presets", "show", "ghost", expect_success=False)
        assert result.exit_code == 1
        assert "unknown preset" in result.output

    def test_preset_download(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "preset-out"
        cli("presets", "add", "anime", "blue_hair solo")
        result = cli(
            "presets", "download", "anime", "--limit", "2", "-o", str(out), "--no-progress"
        )
        assert "Downloaded" in result.output
        assert len(list(out.glob("*.png"))) == 2

    def test_preset_dry_run(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "preset-out"
        cli("presets", "add", "anime", "blue_hair solo")
        result = cli("presets", "download", "anime", "--limit", "2", "-o", str(out), "--dry-run")
        assert "would be downloaded" in result.output
        assert not out.exists()

    def test_preset_limit_is_used_by_default(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "preset-out"
        cli("presets", "add", "anime", "solo", "--limit", "2")
        cli("presets", "download", "anime", "-o", str(out), "--no-progress")
        assert len(list(out.glob("*.png"))) == 2


# --------------------------------------------------------------------------- #
# Config
# --------------------------------------------------------------------------- #
class TestConfigCommands:
    def test_show_lists_settings(self, cli: Callable[..., Any]) -> None:
        result = cli("config", "show")
        assert "general.concurrency" in result.output
        assert "network.base_url" in result.output

    def test_set_and_persist(self, cli: Callable[..., Any]) -> None:
        cli("config", "set", "concurrency", "12")
        assert "12" in cli("config", "show").output
        assert "concurrency = 12" in cli.config.read_text()
        assert cli("config", "show").output.count("12") >= 1

    def test_set_dotted_key(self, cli: Callable[..., Any]) -> None:
        cli("config", "set", "network.user_agent", "Tester/9")
        assert "Tester/9" in cli("config", "show").output

    def test_set_unknown_key_exits_one(self, cli: Callable[..., Any]) -> None:
        result = cli("config", "set", "bogus", "1", expect_success=False)
        assert result.exit_code == 1
        assert "unknown configuration key" in result.output

    def test_set_invalid_value_exits_two(self, cli: Callable[..., Any]) -> None:
        result = cli("config", "set", "concurrency", "many", expect_success=False)
        assert result.exit_code == 2

    def test_unset_restores_default(self, cli: Callable[..., Any]) -> None:
        cli("config", "set", "concurrency", "12")
        cli("config", "unset", "concurrency")
        assert "5" in cli("config", "show").output

    def test_api_key_is_redacted(self, cli: Callable[..., Any]) -> None:
        cli("config", "set", "network.api_key", "super-secret")
        result = cli("config", "show")
        assert "super-secret" not in result.output
        assert "********" in result.output

    def test_path_command(self, cli: Callable[..., Any]) -> None:
        result = cli("config", "path")
        assert "config" in result.output
        assert "history" in result.output

    def test_init_creates_template(self, runner: CliRunner, tmp_path: Path) -> None:
        target = tmp_path / "new" / "config.toml"
        result = runner.invoke(app, ["--config", str(target), "--no-color", "config", "init"])
        assert result.exit_code == 0
        assert target.is_file()
        assert "[general]" in target.read_text()

    def test_init_refuses_to_overwrite(self, cli: Callable[..., Any]) -> None:
        cli.config.write_text("[general]\n")
        result = cli("config", "init", expect_success=False)
        assert result.exit_code == 1

    def test_history_stats(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "solo", "--limit", "2", "-o", str(out), "--no-progress")
        result = cli("config", "history", "--stats")
        assert "Completed" in result.output
        assert "Bytes" in result.output

    def test_history_rejects_unknown_status(self, cli: Callable[..., Any]) -> None:
        result = cli("config", "history", "--status", "bogus", expect_success=False)
        assert result.exit_code == 2

    def test_clear_history_keeps_files(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "downloads"
        cli("download", "solo", "--limit", "1", "-o", str(out), "--no-progress")
        result = cli("config", "clear", "--yes")
        assert "cleared" in result.output
        assert list(out.glob("*.png"))

    def test_config_file_mode_is_private(self, cli: Callable[..., Any]) -> None:
        import stat

        cli("config", "set", "concurrency", "7")
        assert stat.S_IMODE(cli.config.stat().st_mode) == 0o600


# --------------------------------------------------------------------------- #
# Batch and watch
# --------------------------------------------------------------------------- #
class TestBatch:
    def test_batch_reads_a_tag_file(self, cli: Callable[..., Any]) -> None:
        tag_file = cli.tmp_path / "tags.txt"
        tag_file.write_text("# comment\nblue_hair\n\nsolo\n")
        out = cli.tmp_path / "batch-out"
        result = cli(
            "batch",
            str(tag_file),
            "-o",
            str(out),
            "--limit",
            "1",
            "--no-progress",
            "--no-folder-by-line",
        )
        assert "Batch summary" in result.output
        assert "(1/2)" in result.output and "(2/2)" in result.output
        assert len(list(out.glob("*.png"))) == 2
        assert {path.stem for path in out.glob("*.png")} == {
            str(dict(API_REQUESTS[0].params) and 1000 + sum(ord(c) for c in "blue_hair") % 7 * 100),
            str(1000 + sum(ord(c) for c in "solo") % 7 * 100),
        }

    def test_batch_creates_a_folder_per_line(self, cli: Callable[..., Any]) -> None:
        tag_file = cli.tmp_path / "tags.txt"
        tag_file.write_text("blue_hair\n")
        out = cli.tmp_path / "batch-out"
        cli("batch", str(tag_file), "-o", str(out), "--limit", "1", "--no-progress")
        assert (out / "blue_hair").is_dir()

    def test_batch_reports_missing_file(self, cli: Callable[..., Any]) -> None:
        result = cli("batch", "does-not-exist.txt", expect_success=False)
        assert result.exit_code == 2
        assert "not found" in result.output

    def test_batch_reports_empty_file(self, cli: Callable[..., Any]) -> None:
        tag_file = cli.tmp_path / "empty.txt"
        tag_file.write_text("# only comments\n")
        result = cli("batch", str(tag_file), expect_success=False)
        assert result.exit_code == 2

    def test_watch_once(self, cli: Callable[..., Any]) -> None:
        out = cli.tmp_path / "watch-out"
        result = cli("watch", "solo", "--once", "--limit", "2", "-o", str(out), "--no-progress")
        assert "Watch summary" in result.output
        assert len(list(out.glob("*.png"))) == 2


# --------------------------------------------------------------------------- #
# Global options and interactive mode
# --------------------------------------------------------------------------- #
class TestGlobalOptions:
    def test_quiet_suppresses_the_banner(self, runner: CliRunner, tmp_path: Path) -> None:
        config = tmp_path / "config.toml"
        noisy = runner.invoke(
            app, ["--config", str(config), "--no-color", "search", "solo", "--limit", "1"]
        )
        quiet = runner.invoke(
            app,
            ["--config", str(config), "--no-color", "--quiet", "search", "solo", "--limit", "1"],
        )
        assert "gelbooru" in noisy.output
        assert "____" not in quiet.output

    def test_show_banner_false(self, cli: Callable[..., Any]) -> None:
        cli("config", "set", "display.show_banner", "false")
        result = cli("search", "solo", "--limit", "1")
        assert "____" not in result.output

    def test_color_flag(self, cli: Callable[..., Any]) -> None:
        result = cli("search", "solo", "--limit", "1")
        assert "\x1b[" not in result.output  # --no-color honoured

    def test_table_max_rows(self, cli: Callable[..., Any]) -> None:
        cli("config", "set", "display.table_max_rows", "2")
        result = cli("search", "solo", "--limit", "5")
        assert "showing 2 of 5" in result.output

    def test_log_file_option(self, runner: CliRunner, tmp_path: Path) -> None:
        config = tmp_path / "config.toml"
        log = tmp_path / "logs" / "gelbooru.log"
        result = runner.invoke(
            app, ["--config", str(config), "--no-color", "-vv", "--log-file", str(log), "version"]
        )
        assert result.exit_code == 0
        assert log.exists()


class TestInteractive:
    def test_interactive_help_and_quit(self, runner: CliRunner, tmp_path: Path) -> None:
        config = tmp_path / "config.toml"
        result = runner.invoke(
            app, ["--config", str(config), "--no-color", "interactive"], input="help\nquit\n"
        )
        assert result.exit_code == 0
        assert "commands" in result.output
        assert "bye" in result.output

    def test_interactive_search(self, runner: CliRunner, tmp_path: Path) -> None:
        config = tmp_path / "config.toml"
        result = runner.invoke(
            app,
            ["--config", str(config), "--no-color", "interactive"],
            input="search blue_hair -n 2\nquit\n",
        )
        assert result.exit_code == 0
        assert "matched" in result.output

    def test_interactive_unknown_command(self, runner: CliRunner, tmp_path: Path) -> None:
        config = tmp_path / "config.toml"
        result = runner.invoke(
            app, ["--config", str(config), "--no-color", "interactive"], input="wat\nquit\n"
        )
        assert "unknown command" in result.output

    def test_interactive_download(self, runner: CliRunner, tmp_path: Path) -> None:
        config = tmp_path / "config.toml"
        out = tmp_path / "idl"
        result = runner.invoke(
            app,
            ["--config", str(config), "--no-color", "--quiet", "interactive"],
            input=f"download solo -n 2 -o {out}\nquit\n",
        )
        assert result.exit_code == 0
        assert len(list(out.glob("*.png"))) == 2
