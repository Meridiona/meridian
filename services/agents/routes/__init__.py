"""FastAPI route modules for the Meridian agent server.

Each module owns one endpoint group and exposes a `router` (APIRouter) that
`agents.server` wires in via `include_router`. Modules are agent-blind — they
read shared process state from `agents._state` and never import each other.
"""
