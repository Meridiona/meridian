# Stub package — the upstream hermes runtime ships richer impls of these
# environment backends (local/singularity/ssh/docker/modal). Meridian's
# synthesizer runs with `enabled_toolsets=[]`, so terminal/code-execution
# tools are never invoked; we just need the names to be importable.
