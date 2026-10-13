"""Provider error sanitization shared by daemon-leased model requests."""

from pydantic_ai.exceptions import ModelAPIError, ModelHTTPError

from .errors import AuthUnavailable

CODEX_URL = "https://chatgpt.com/backend-api/codex"
PROVIDER_URLS = {
    "openai-codex": CODEX_URL,
    "anthropic": "https://api.anthropic.com",
    "github-copilot": "https://api.githubcopilot.com",
}


def sanitized_error(error: Exception, model_name: str) -> Exception:
    cause: BaseException | None = error
    seen: set[int] = set()
    while cause is not None and id(cause) not in seen:
        seen.add(id(cause))
        if isinstance(cause, AuthUnavailable):
            return cause
        cause = cause.__cause__
    if isinstance(error, ModelHTTPError):
        return ModelHTTPError(
            error.status_code, model_name, body="provider rejected request"
        )
    return ModelAPIError(
        model_name, "Provider request failed; response details redacted"
    )
