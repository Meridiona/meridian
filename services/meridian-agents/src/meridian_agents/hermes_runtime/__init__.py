# meridian — normalises screenpipe activity into structured app sessions
#
# Vendored runtime pieces from hermes-activity-agent (MIT-licensed; see
# ../../../reference/LICENSE.hermes for upstream attribution).
#
# Hermes uses ABSOLUTE imports throughout — `from hermes_constants import X`,
# `from agent.error_classifier import Y`, etc. To make those resolve without
# rewriting every import in every vendored file, we splice this directory
# onto sys.path the first time the package is imported. That lets the
# vendored code keep its original layout (and stay diffable against
# upstream) while the rest of meridian-agents addresses these modules via
# `meridian_agents.hermes_runtime.<name>`.
#
# WARNING: this means top-level names like `agent`, `tools`, `hermes_cli`,
# `utils`, `hermes_constants`, `model_tools` will be importable globally
# inside this Python process once meridian-agents starts up. Don't pip
# install anything that collides with those names.

import os
import sys

_HERMES_DIR = os.path.dirname(os.path.abspath(__file__))
if _HERMES_DIR not in sys.path:
    sys.path.insert(0, _HERMES_DIR)
