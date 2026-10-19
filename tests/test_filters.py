"""Client-side filtering, tag-expression building and result ordering."""

from __future__ import annotations

import pytest

from gelbooru_downloader.commands._common import build_search_tags, split_csv
from gelbooru_downloader.filters import (
    PostFilter,
    build_filter,
    folder_name_for,
    sort_key,
)
from gelbooru_downloader.models import DownloadStatus, Post

from .conftest import make_post


class TestRatingFilter:
    def test_accepts_matching_rating(self) -> None:
        post_filter = build_filter(ratings=["safe"])
        assert post_filter.matches(make_post(rating="safe"))

    def test_rejects_other_rating(self) -> None:
        post_filter = build_filter(ratings=["safe"])
        reason = post_filter.reject_reason(make_post(rating="explicit"))
        assert reason is not None
        assert "rating" in reason

    def test_accepts_any_of_several(self) -> None:
        post_filter = build_filter(ratings=["safe", "questionable"])
        assert post_filter.matches(make_post(rating="safe"))
        assert post_filter.matches(make_post(rating="questionable"))
        assert not post_filter.matches(make_post(rating="explicit"))

    def test_unknown_rating_value_is_tolerated(self) -> None:
        post_filter = build_filter(ratings=["weird"])
        assert not post_filter.matches(make_post(rating="safe"))

    def test_case_insensitive(self) -> None:
        assert build_filter(ratings=["SAFE"]).matches(make_post(rating="safe"))

    def test_general_rating_from_the_live_api(self) -> None:
        # Over half of current Gelbooru posts are rated "general".
        post_filter = build_filter(ratings=["general"])
        assert post_filter.matches(make_post(rating="general"))
        assert not post_filter.matches(make_post(rating="safe"))


class TestTagFilter:
    def test_excluded_tag_is_rejected(self) -> None:
        post_filter = build_filter(exclude_tags=["nude"])
        reason = post_filter.reject_reason(make_post(tags="solo nude"))
        assert reason is not None
        assert "nude" in reason

    def test_other_tags_pass(self) -> None:
        assert build_filter(exclude_tags=["nude"]).matches(make_post(tags="solo clothed"))

    def test_tag_matching_is_case_insensitive(self) -> None:
        assert not build_filter(exclude_tags=["Nude"]).matches(make_post(tags="nude"))

    def test_filter_lists_rejected_posts_with_reasons(self) -> None:
        posts = [make_post(id="1", tags="solo"), make_post(id="2", tags="solo nude")]
        kept, rejected = build_filter(exclude_tags=["nude"]).filter_with_reasons(posts)
        assert [post.id for post in kept] == [1]
        assert rejected[0][0].id == 2
        assert "nude" in rejected[0][1]


class TestDimensionFilter:
    @pytest.mark.parametrize(
        ("kwargs", "width", "height", "expected"),
        [
            ({"min_width": 1000}, 1920, 1080, True),
            ({"min_width": 4000}, 1920, 1080, False),
            ({"max_width": 1000}, 1920, 1080, False),
            ({"min_height": 2000}, 1920, 1080, False),
            ({"max_height": 2000}, 1920, 1080, True),
            ({"min_pixels": 5_000_000}, 1920, 1080, False),
            ({"max_pixels": 1_000_000}, 1920, 1080, False),
            ({"max_pixels": 4_000_000}, 1920, 1080, True),
        ],
    )
    def test_dimension_rules(self, kwargs: dict, width: int, height: int, expected: bool) -> None:
        post_filter = build_filter(**kwargs)
        post = make_post(width=str(width), height=str(height))
        assert post_filter.matches(post) is expected

    def test_missing_dimensions_are_not_filtered_out(self) -> None:
        # Gelbooru always reports dimensions for live posts; when it does not we
        # cannot prove a violation, so the post is kept rather than silently lost.
        post = make_post(width="", height="")
        assert build_filter(min_width=100).matches(post) is True
        assert build_filter(max_width=100).matches(post) is True


class TestSizeFilter:
    def test_min_size(self) -> None:
        post_filter = build_filter(min_size=1000)
        assert post_filter.matches(make_post(file_size="2000"))
        assert not post_filter.matches(make_post(file_size="500"))

    def test_max_size(self) -> None:
        post_filter = build_filter(max_size=1000)
        assert not post_filter.matches(make_post(file_size="2000"))

    def test_unknown_size_is_kept(self) -> None:
        post = make_post(file_size="")
        assert post.file_size is None
        assert build_filter(min_size=1000).matches(post)


class TestExtensionFilter:
    def test_extension_match(self) -> None:
        post_filter = build_filter(extensions=["jpg"])
        assert post_filter.matches(make_post(type="jpg"))
        assert not post_filter.matches(make_post(type="png"))

    def test_dotted_and_uppercase_input(self) -> None:
        assert build_filter(extensions=[".JPG"]).matches(make_post(type="jpg"))

    def test_extension_inferred_from_file_name(self) -> None:
        post = make_post(type="", file_name="1234.gif")
        assert build_filter(extensions=["gif"]).matches(post)

    def test_score_filter(self) -> None:
        assert build_filter(min_score=50).matches(make_post(score="60"))
        assert not build_filter(min_score=50).matches(make_post(score="10"))


class TestAvailability:
    def test_unavailable_posts_are_rejected_by_default(self) -> None:
        post = make_post(file_url="", status="deleted")
        reason = build_filter().reject_reason(post)
        assert reason is not None

    def test_availability_check_can_be_disabled(self) -> None:
        post = make_post(file_url="", status="deleted")
        post_filter = build_filter(require_available=False)
        assert post_filter.matches(post)


class TestFilterMechanics:
    def test_empty_filter_matches_everything(self) -> None:
        post_filter = PostFilter(require_available=False)
        assert bool(post_filter) is False
        assert post_filter.matches(make_post())

    def test_apply_filters_a_list(self) -> None:
        posts = [make_post(id="1", rating="safe"), make_post(id="2", rating="explicit")]
        assert [post.id for post in build_filter(ratings=["safe"]).apply(posts)] == [1]

    def test_matches_all_helper(self) -> None:
        from gelbooru_downloader.filters import matches_all

        posts = [make_post(id="1", rating="safe"), make_post(id="2", rating="explicit")]
        kept = list(matches_all(posts, build_filter(ratings=["explicit"])))
        assert [post.id for post in kept] == [2]

    def test_filter_is_frozen(self) -> None:
        from pydantic import ValidationError  # noqa: F401 - documents the runtime error type

        post_filter = build_filter(ratings=["safe"])
        with pytest.raises(AttributeError):
            post_filter.ratings = frozenset()  # type: ignore[misc]


class TestFolders:
    def test_folder_from_leading_tags(self) -> None:
        post = make_post(tags="blue_hair solo long_hair rating:safe")
        assert folder_name_for(post) == "blue_hair_solo"

    def test_fallback_when_no_usable_tags(self) -> None:
        assert folder_name_for(make_post(tags="rating:safe")) == "untagged"
        assert folder_name_for(make_post(tags="")) == "untagged"

    def test_custom_tag_count(self) -> None:
        post = make_post(tags="a b c")
        assert folder_name_for(post, max_tags=3) == "a_b_c"


class TestSortKey:
    def test_default_is_newest_first(self) -> None:
        posts = [make_post(id="1"), make_post(id="5"), make_post(id="3")]

        def by_default(post: Post) -> tuple:
            return sort_key(post)

        assert [post.id for post in sorted(posts, key=by_default)] == [5, 3, 1]

    def test_score_desc(self) -> None:
        posts = [make_post(id="1", score="5"), make_post(id="2", score="50")]

        def by_score(post: Post) -> tuple:
            return sort_key(post, "score")

        assert [post.id for post in sorted(posts, key=by_score)] == [2, 1]

    def test_filesize_and_width(self) -> None:
        small = make_post(id="1", file_size="100", width="10")
        large = make_post(id="2", file_size="900", width="900")
        assert [
            p.id for p in sorted([small, large], key=lambda post: sort_key(post, "filesize"))
        ] == [2, 1]
        assert [p.id for p in sorted([small, large], key=lambda p: sort_key(p, "width"))] == [2, 1]

    def test_unknown_order_falls_back_to_id(self) -> None:
        posts = [make_post(id="1"), make_post(id="2")]
        assert [p.id for p in sorted(posts, key=lambda p: sort_key(p, "nonsense"))] == [2, 1]


class TestSearchTagBuilding:
    def test_plain_tags_pass_through(self) -> None:
        assert build_search_tags("blue_hair solo") == "blue_hair solo"

    def test_ratings_become_metatags(self) -> None:
        assert build_search_tags("solo", ratings=["safe"]) == "solo rating:safe"

    def test_exclusions_are_negated(self) -> None:
        assert build_search_tags("solo", exclude=["nude", "blurry"]) == "solo -nude -blurry"

    def test_inline_exclusions_are_preserved(self) -> None:
        assert build_search_tags("solo -nude") == "solo -nude"

    def test_order_metatag(self) -> None:
        assert build_search_tags("solo", order="score") == "solo order:score"

    def test_no_duplicates(self) -> None:
        built = build_search_tags(
            "solo", ratings=["safe", "safe"], exclude=["nude", "nude"], order="score"
        )
        assert built.count("rating:safe") == 1
        assert built.count("-nude") == 1
        assert built.count("order:score") == 1

    def test_split_csv(self) -> None:
        assert split_csv(["a,b", " c ", ""]) == ["a", "b", "c"]
        assert split_csv(["-nude,!blurry"]) == ["nude", "blurry"]
        assert split_csv(None) == []


class TestPlanQueries:
    def test_single_query_without_ratings(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo") == ["solo"]
        assert plan_queries("") == [""]

    def test_single_rating_makes_one_query(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo", ratings=["safe"]) == ["solo rating:safe"]

    def test_comma_separated_ratings_become_separate_queries(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo", ratings=["questionable,explicit"]) == [
            "solo rating:questionable",
            "solo rating:explicit",
        ]

    def test_repeated_rating_flags_are_ored(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo", ratings=["safe", "general"]) == [
            "solo rating:safe",
            "solo rating:general",
        ]

    def test_case_and_whitespace_are_normalised(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo", ratings=[" Safe , EXPLICIT "]) == [
            "solo rating:safe",
            "solo rating:explicit",
        ]

    def test_duplicates_collapse(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo", ratings=["safe,safe"]) == ["solo rating:safe"]

    def test_exclude_and_order_are_applied_to_every_query(self) -> None:
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo", ratings=["safe,general"], exclude=["nude"], order="score") == [
            "solo -nude order:score rating:safe",
            "solo -nude order:score rating:general",
        ]

    def test_inline_negated_rating_does_not_create_a_query(self) -> None:
        # "-rating:safe" is handled inside the tag expression, not as an OR.
        from gelbooru_downloader.commands._common import plan_queries

        assert plan_queries("solo -rating:safe", ratings=["general"]) == [
            "solo -rating:safe rating:general",
        ]


class TestPostModel:
    def test_from_api_coerces_and_parses(self) -> None:
        post = Post.from_api(
            {"id": "9", "file_size": "12.5", "width": "100", "tags": "  a   b  ", "rating": "SAFE"}
        )
        assert post.id == 9
        assert post.file_size == 12
        assert post.rating_value.value == "safe"
        assert post.tag_list == ["a", "b"]

    def test_missing_id_raises_value_error(self) -> None:
        with pytest.raises(ValueError, match="without a valid id"):
            Post.from_api({"file_url": "https://x.invalid/a.png"})

    def test_non_dict_raises(self) -> None:
        with pytest.raises(ValueError, match="expected a JSON object"):
            Post.from_api(["not", "a", "dict"])

    def test_bool_values_are_not_treated_as_ints(self) -> None:
        assert Post.from_api({"id": "1", "is_pending": True}).is_pending is True

    def test_created_at_conversion(self) -> None:
        assert make_post(created_at="1700000000").created_at_dt is not None
        assert Post.from_api({"id": "1"}).created_at_dt is None

    def test_summary_truncates_tags(self) -> None:
        post = make_post(tags=" ".join(f"tag{i}" for i in range(10)) + " rating:safe")
        summary = post.summary(3)
        assert summary.endswith("…")
        assert "rating:safe" not in summary

    def test_to_dict_marks_availability(self) -> None:
        assert make_post().to_dict()["available"] is True
        assert make_post(file_url="").to_dict()["available"] is False

    def test_media_kind(self) -> None:
        assert make_post(type="jpg").media_kind == "image"
        assert make_post(type="webm").media_kind == "video"

    def test_download_result_describe(self) -> None:
        from gelbooru_downloader.models import DownloadResult

        completed = DownloadResult(post_id=1, status=DownloadStatus.COMPLETED, path="/tmp/a.png")
        assert completed.describe() == "1 -> /tmp/a.png"
        assert (
            "already downloaded"
            in DownloadResult(post_id=1, status=DownloadStatus.SKIPPED).describe()
        )
        assert (
            "failed"
            in DownloadResult(post_id=1, status=DownloadStatus.FAILED, error="x").describe()
        )
