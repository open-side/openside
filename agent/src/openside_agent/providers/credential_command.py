import asyncio
import os
import signal
from collections.abc import Sequence

from .errors import AuthUnavailable

MAX_KEY_BYTES = 16 * 1024
CLEANUP_SECONDS = 1.0


async def key_bytes(argv: Sequence[str], provider: str, timeout: float) -> bytes:
    if os.name != "posix":
        raise AuthUnavailable(
            provider, ["command: process-group isolation unavailable"]
        )
    try:
        process = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
            start_new_session=True,
            limit=MAX_KEY_BYTES + 1,
        )
    except (OSError, ValueError):
        raise AuthUnavailable(provider, ["command: executable unavailable"]) from None

    async def read() -> bytes:
        if process.stdout is None:
            raise ValueError
        output = bytearray()
        while chunk := await process.stdout.read(4096):
            if len(output) + len(chunk) > MAX_KEY_BYTES:
                raise ValueError
            output.extend(chunk)
        if await process.wait() != 0:
            raise ValueError
        return bytes(output)

    async def stop() -> None:
        # The direct child can exit while a descendant still owns stdout. Its session
        # group, not returncode, decides whether there is anything left to terminate.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            await asyncio.wait_for(process.wait(), CLEANUP_SECONDS)
        except TimeoutError:
            pass

    try:
        return await asyncio.wait_for(read(), min(60.0, max(0.001, timeout)))
    except (TimeoutError, ValueError, OSError):
        raise AuthUnavailable(
            provider, ["command: failed or exceeded limits; output redacted"]
        ) from None
    finally:
        await asyncio.shield(stop())
