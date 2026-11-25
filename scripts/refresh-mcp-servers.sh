#!/usr/bin/env bash
# Verify crates/nika-catalog/data/mcp-servers.toml entry by entry.
#
# The MCP shelf is a VENDORED census of third-party servers, refreshed at
# maintenance time and never fetched at run time. Each entry's packages are
# probed on their own registry (npm, PyPI, OCI) and each remote receives an
# MCP `initialize`; the official MCP Registry is queried as a cross-check. A
# fully verified entry gets `last_verified = <today>`; every other entry keeps
# its older date, and the generated [meta] region records the counts.
#
# Usage:
#   bash scripts/refresh-mcp-servers.sh               # probe, write dates + [meta]
#   bash scripts/refresh-mcp-servers.sh --check       # probe, report, write nothing
#   bash scripts/refresh-mcp-servers.sh --json FILE   # also save the JSON report
#   python3 scripts/mcp-servers/refresh_mcp_servers.py --selftest
#
# Exit 0 · every entry verified · 2 · environment · 4 · an entry is dead or
# deprecated upstream (named in the report; nothing is removed automatically).
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/mcp-servers/refresh_mcp_servers.py "$@"
