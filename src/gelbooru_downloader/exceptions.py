"""Exception hierarchy for the Gelbooru downloader.

Every error raised on purpose by this package derives from :class:`GelbooruError`,
which lets the CLI turn them into friendly, coloured messages instead of
tracebacks.
"""

from __future__ import annotations

__all__ = [
    "ApiError",
    "AuthenticationError",
    "ConfigurationError",
    "ContentRejectedError",
    "DownloadError",
    "DownloadUnavailableError",
    "GelbooruError",
    "HistoryError",
    "MalformedResponseError",
    "NetworkError",
    "NotFoundError",
    "RateLimitError",
    "StorageError",
]


class GelbooruError(Exception):
    """Base class for all errors raised by this application."""

    def __init__(self, message: str, *, hint: str | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.hint = hint

    def __str__(self) -> str:
        return self.message


class ConfigurationError(GelbooruError):
    """Raised when configuration values are missing or invalid."""


class StorageError(GelbooruError):
    """Raised when the local database or config file cannot be read/written."""


class HistoryError(StorageError):
    """Raised when the download history database operation fails."""


class ApiError(GelbooruError):
    """Raised for any non-fatal problem while talking to the Gelbooru API."""

    def __init__(
        self,
        message: str,
        *,
        status_code: int | None = None,
        url: str | None = None,
        hint: str | None = None,
    ) -> None:
        super().__init__(message, hint=hint)
        self.status_code = status_code
        self.url = url


class AuthenticationError(ApiError):
    """Raised on HTTP 401/403 — Gelbooru requires API credentials for the dapi."""

    default_hint = (
        "Gelbooru's dapi requires credentials. Create an API key at "
        "https://gelbooru.com/account and set GELBOORU_USER_ID / GELBOORU_API_KEY "
        "(or run `gelbooru config set network.api_key ...`)."
    )

    def __init__(
        self,
        message: str = "Gelbooru rejected the request: authentication required (HTTP 401).",
        *,
        status_code: int | None = None,
        url: str | None = None,
        hint: str | None = None,
    ) -> None:
        super().__init__(message, status_code=status_code, url=url, hint=hint or self.default_hint)


class RateLimitError(ApiError):
    """Raised on HTTP 429 (and on 403 responses that carry a rate-limit hint)."""

    def __init__(
        self,
        message: str = "Rate limited by Gelbooru (HTTP 429).",
        *,
        retry_after: float | None = None,
        status_code: int | None = None,
        url: str | None = None,
    ) -> None:
        super().__init__(message, status_code=status_code, url=url)
        self.retry_after = retry_after


class NotFoundError(ApiError):
    """Raised when a post id does not exist."""


class MalformedResponseError(ApiError):
    """Raised when the API returns something that cannot be parsed."""


class NetworkError(ApiError):
    """Raised for connection failures, DNS problems and timeouts."""


class DownloadError(GelbooruError):
    """Raised when a single image download fails."""

    def __init__(
        self,
        message: str,
        *,
        post_id: int | None = None,
        url: str | None = None,
        hint: str | None = None,
    ) -> None:
        super().__init__(message, hint=hint)
        self.post_id = post_id
        self.url = url
        self.status_code: int | None = None
        #: Filled in by the downloader's retry loop so history rows keep the count.
        self.attempts: int = 0


class DownloadUnavailableError(DownloadError):
    """Raised when the file for a post is missing or has been removed."""


class ContentRejectedError(DownloadError):
    """Raised when the server answers with something that is not an image."""
