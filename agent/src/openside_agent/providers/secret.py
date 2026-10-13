"""Owned mutable token buffers with redacted representations."""


class CredentialSecret:
    __slots__ = ("_buffer",)

    def __init__(self, value: str) -> None:
        self._buffer = bytearray(value, "utf-8")

    def __repr__(self) -> str:
        return "CredentialSecret(**********)"

    def reveal(self) -> str:
        return self._buffer.decode("utf-8")

    def clear(self) -> None:
        self._buffer[:] = b"\x00" * len(self._buffer)
        self._buffer.clear()

    def __del__(self) -> None:
        self.clear()
