"""Read non-secret shaping and daemon-provided diagnostic configuration."""

from collections.abc import Mapping
from typing import Any, cast


def section(config: Mapping[str, Any], name: str) -> Mapping[str, Any]:
    value = config.get(name, {})
    return cast(Mapping[str, Any], value) if isinstance(value, Mapping) else {}
