# meridian — normalises screenpipe activity into structured app sessions

"""Shared stub helpers for tools.environments.* modules.

If meridian-agents ever attempts to instantiate or use one of these
classes/functions at runtime, the stub raises a clear NotImplementedError
pointing back here. Adjust if/when the synthesizer actually needs one
of these environments.
"""


class _StubEnvironment:
    """Placeholder for hermes' real environment classes (Local, SSH, Docker,
    Modal, Singularity, ManagedModal). meridian-agents does not run shell
    commands from agents, so these are unreachable in our flow."""

    def __init__(self, *args, **kwargs):
        raise NotImplementedError(
            f"{type(self).__name__} is a meridian-agents stub. "
            "tools/environments/ is missing in the hermes upstream snapshot we "
            "vendored from. If you need this class, restore the real "
            "implementation from a complete hermes distribution."
        )

    def __getattr__(self, name):
        raise NotImplementedError(
            f"{type(self).__name__}.{name} is a meridian-agents stub. "
            "Restore the real implementation from a complete hermes copy."
        )
