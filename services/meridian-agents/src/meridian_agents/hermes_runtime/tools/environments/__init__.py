# meridian — normalises screenpipe activity into structured app sessions
#
# STUB PACKAGE — `tools/environments/` is referenced by upstream hermes
# (terminal_tool.py, ai_agent.py, etc.) but the implementation is NOT
# present in the hermes-activity-agent repo we vendored from. The
# directory simply doesn't exist upstream.
#
# meridian-agents never exercises any environment class (we don't run
# shell, browser, or modal code from agents — only LLM tool calls into
# our own Python helpers). These stubs exist only to satisfy module-load
# imports; calling any method on them raises NotImplementedError so it's
# loud if we ever wander into this code path by accident.
