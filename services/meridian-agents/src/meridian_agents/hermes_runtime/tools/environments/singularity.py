# meridian — normalises screenpipe activity into structured app sessions
# STUB — see tools/environments/__init__.py for context.

from pathlib import Path

from tools.environments._stub import _StubEnvironment


class SingularityEnvironment(_StubEnvironment):
    pass


def _get_scratch_dir() -> Path:
    """Stub: returns /tmp. Real hermes computes a per-job scratch dir
    inside a Singularity container; meridian-agents never invokes this."""
    return Path("/tmp")
