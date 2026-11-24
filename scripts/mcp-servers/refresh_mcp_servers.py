#!/usr/bin/env python3
# refresh_mcp_servers.py — per-entry verification of crates/nika-catalog/data/mcp-servers.toml.
#
#   bash scripts/refresh-mcp-servers.sh               # probe every entry, write the dates + [meta]
#   bash scripts/refresh-mcp-servers.sh --check       # probe, report, write nothing
#   bash scripts/refresh-mcp-servers.sh --json FILE   # also save the full report as JSON
#   python3 scripts/mcp-servers/refresh_mcp_servers.py --selftest
#
# Exit 0 · every entry verified (or written)
# Exit 2 · environment (the file is unreadable or malformed)
# Exit 4 · at least one entry is DEAD (a package or remote its registry says is gone) or
#          DEPRECATED upstream; nothing is ever removed here, the report names each one
#
# The shelf describes third-party products, so its contract is "this exists, according to
# THIS source, on THIS date". Each entry is probed where it is distributed:
#   - npm   · GET registry.npmjs.org/<pkg>/latest  (404 = gone; a `deprecated` field on the
#             latest version = deprecated upstream)
#   - pypi  · GET pypi.org/pypi/<pkg>/json         (404 = gone)
#   - oci   · an anonymous pull token, then HEAD the `latest` manifest on ghcr.io or
#             Docker Hub (404 = gone; another registry is unknown)
#   - remote · POST a JSON-RPC `initialize` to the URL: 2xx, or 401/403 (the server answered
#             and asks for its auth) = alive; 404/410 = gone; anything else = unknown
#   - mcpb  · no online registry: unknown
# An entry is VERIFIED when every package and remote it declares answers alive and none is
# deprecated. Only a verified entry gets `last_verified = <today>`; every other entry keeps
# the date it had, so the date is a per-entry measurement, never a batch stamp. The official
# MCP Registry (registry.modelcontextprotocol.io) is queried as a cross-check: an entry whose
# package or remote is listed there is reported with the registry's server name. Network
# failures are UNKNOWN, never DEAD: only an authoritative 404 condemns a package.
from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import json
import pathlib
import re
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
TARGET = ROOT / "crates/nika-catalog/data/mcp-servers.toml"
REGISTRY = "https://registry.modelcontextprotocol.io/v0/servers"
USER_AGENT = "nika-catalog-refresh (+https://github.com/nika-open/nika)"
TIMEOUT = 20
META_BEGIN = "# BEGIN generated [meta] by scripts/refresh-mcp-servers.sh"
META_END = "# END generated [meta]"
SOURCES = (
    "registry.npmjs.org · pypi.org · ghcr.io / registry-1.docker.io · "
    "remote MCP initialize · registry.modelcontextprotocol.io (cross-check)"
)

ALIVE, GONE, DEPRECATED, UNKNOWN = "alive", "gone", "deprecated", "unknown"


# ── probes ─────────────────────────────────────────────────────────────────────────────────


def _request(url, method="GET", body=None, headers=None):
    """(status, parsed-json-or-None). A transport failure raises OSError."""
    req = urllib.request.Request(url, data=body, method=method)
    req.add_header("User-Agent", USER_AGENT)
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            raw = resp.read(2_000_000)
            status = resp.status
    except urllib.error.HTTPError as e:
        return e.code, None
    try:
        return status, json.loads(raw) if raw else None
    except ValueError:
        return status, None


def probe_npm(pkg):
    name = pkg.replace("/", "%2F", 1) if pkg.startswith("@") else pkg
    status, doc = _request(f"https://registry.npmjs.org/{name}/latest")
    if status == 404:
        return GONE, "npm 404"
    if status != 200 or not isinstance(doc, dict):
        return UNKNOWN, f"npm HTTP {status}"
    version = doc.get("version", "?")
    if doc.get("deprecated"):
        return DEPRECATED, f"npm {version} deprecated: {str(doc['deprecated'])[:120]}"
    return ALIVE, f"npm {version}"


def probe_pypi(pkg):
    status, doc = _request(f"https://pypi.org/pypi/{urllib.parse.quote(pkg)}/json")
    if status == 404:
        return GONE, "pypi 404"
    if status != 200 or not isinstance(doc, dict):
        return UNKNOWN, f"pypi HTTP {status}"
    return ALIVE, f"pypi {doc.get('info', {}).get('version', '?')}"


def probe_oci(identifier):
    image, _, tag = identifier.partition(":")
    tag = tag or "latest"
    registry, _, repo = image.partition("/")
    if registry == "ghcr.io":
        token_url = f"https://ghcr.io/token?scope=repository:{repo}:pull"
        manifest = f"https://ghcr.io/v2/{repo}/manifests/{tag}"
    elif registry in ("docker.io", "registry-1.docker.io"):
        repo = repo if "/" in repo else f"library/{repo}"
        token_url = (
            "https://auth.docker.io/token?service=registry.docker.io"
            f"&scope=repository:{repo}:pull"
        )
        manifest = f"https://registry-1.docker.io/v2/{repo}/manifests/{tag}"
    else:
        return UNKNOWN, f"oci registry {registry} not probed"
    status, doc = _request(token_url)
    token = (doc or {}).get("token") if status == 200 else None
    if not token:
        return UNKNOWN, f"oci token HTTP {status}"
    accept = ", ".join(
        [
            "application/vnd.oci.image.index.v1+json",
            "application/vnd.oci.image.manifest.v1+json",
            "application/vnd.docker.distribution.manifest.list.v2+json",
            "application/vnd.docker.distribution.manifest.v2+json",
        ]
    )
    status, _ = _request(
        manifest, "HEAD", headers={"Authorization": f"Bearer {token}", "Accept": accept}
    )
    if status == 200:
        return ALIVE, f"oci {tag}"
    if status == 404:
        return GONE, f"oci manifest {tag} 404"
    return UNKNOWN, f"oci HTTP {status}"


def probe_remote(url):
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "nika-catalog-refresh", "version": "1"},
            },
        }
    ).encode()
    status, _ = _request(
        url,
        "POST",
        body,
        {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
    )
    if 200 <= status < 300:
        return ALIVE, f"remote HTTP {status}"
    if status in (401, 403):
        return ALIVE, f"remote HTTP {status} (answers, asks for its auth)"
    if status in (404, 410):
        return GONE, f"remote HTTP {status}"
    return UNKNOWN, f"remote HTTP {status}"


def probe_target(kind, ident):
    try:
        if kind == "npm":
            return probe_npm(ident)
        if kind == "pypi":
            return probe_pypi(ident)
        if kind == "oci":
            return probe_oci(ident)
        if kind == "remote":
            return probe_remote(ident)
        return UNKNOWN, f"{kind}: no online registry"
    except (OSError, ValueError) as e:  # DNS, TLS, timeout: never a verdict on the package
        return UNKNOWN, f"{kind} network: {type(e).__name__}"


def registry_listing(server):
    """The official MCP Registry server name listing one of this entry's packages or remotes."""
    idents = {p["identifier"].split(":")[0] if p["registry_type"] == "oci" else p["identifier"]
              for p in server.get("packages", [])}
    urls = {r["url"].rstrip("/") for r in server.get("remotes", [])}
    terms = {server["id"]}
    terms.update(i.rsplit("/", 1)[-1] for i in idents)
    for term in sorted(terms):
        query = urllib.parse.urlencode({"search": term, "limit": 100})
        try:
            status, doc = _request(f"{REGISTRY}?{query}")
        except (OSError, ValueError):
            return None
        if status != 200 or not isinstance(doc, dict):
            continue
        for row in doc.get("servers", []):
            s = row.get("server", {})
            pk = {
                p.get("identifier", "").split(":")[0]
                if p.get("registryType") == "oci" else p.get("identifier", "")
                for p in s.get("packages") or []
            }
            rm = {r.get("url", "").rstrip("/") for r in s.get("remotes") or []}
            if idents & pk or urls & rm:
                return s.get("name")
    return None


# ── classification ─────────────────────────────────────────────────────────────────────────


def targets(server):
    out = [(p["registry_type"], p["identifier"]) for p in server.get("packages", [])]
    out += [("remote", r["url"]) for r in server.get("remotes", [])]
    return out


def classify(results):
    """One entry's state from its probes: dead beats deprecated beats unknown beats verified."""
    states = [state for _, _, state, _ in results]
    if GONE in states:
        return "dead"
    if DEPRECATED in states:
        return "deprecated"
    if UNKNOWN in states or not states:
        return "unknown"
    return "verified"


def check_entry(server, with_registry):
    results = [(k, i, *probe_target(k, i)) for k, i in targets(server)]
    listed = registry_listing(server) if with_registry else None
    return {
        "id": server["id"],
        "state": classify(results),
        "probes": [{"kind": k, "target": i, "state": s, "detail": d} for k, i, s, d in results],
        "mcp_registry": listed,
    }


# ── write-back ─────────────────────────────────────────────────────────────────────────────

LAST_VERIFIED = re.compile(r'^last_verified = "[0-9]{4}-[0-9]{2}-[0-9]{2}"$')
ID_LINE = re.compile(r'^id = "([^"]+)"$')


def stamp(text, verified_ids, today):
    """Rewrite `last_verified` of each verified entry; every other line is kept byte for byte."""
    out, current = [], None
    for line in text.split("\n"):
        if line == "[[servers]]":
            current = None
        m = ID_LINE.match(line)
        if m and current is None:
            current = m.group(1)
        if LAST_VERIFIED.match(line) and current in verified_ids:
            line = f'last_verified = "{today}"'
        out.append(line)
    return "\n".join(out)


def meta_block(report, dates, today):
    counts = {s: sum(1 for r in report if r["state"] == s)
              for s in ("verified", "dead", "deprecated", "unknown")}
    listed = sum(1 for r in report if r["mcp_registry"])
    return "\n".join([
        META_BEGIN,
        "[meta]",
        'shelf = "census"',
        'last_verified_semantics = "per-entry"',
        f'as_of = "{today}"',
        f'source = "{SOURCES}"',
        'refresh = "bash scripts/refresh-mcp-servers.sh"',
        f"entries = {len(report)}",
        f"verified = {counts['verified']}",
        f"dead = {counts['dead']}",
        f"deprecated_upstream = {counts['deprecated']}",
        f"unknown = {counts['unknown']}",
        f"listed_in_mcp_registry = {listed}",
        f"distinct_last_verified_values = {len(dates)}",
        "source_field_present = false",
        "staleness_budget_days = 120",
        'gate = "ABSENT"',
        'upstream = "registry.modelcontextprotocol.io"',
        META_END,
    ])


def replace_meta(text, block):
    begin, end = text.find(META_BEGIN), text.find(META_END)
    if begin < 0 or end < begin:
        raise ValueError(f"{TARGET.name}: no generated [meta] region ({META_BEGIN!r} … {META_END!r})")
    return text[:begin] + block + text[end + len(META_END):]


# ── main ───────────────────────────────────────────────────────────────────────────────────


def run(args):
    try:
        text = TARGET.read_text()
        servers = tomllib.loads(text)["servers"]
    except (OSError, ValueError, KeyError) as e:
        print(f"error: cannot read {TARGET}: {e}", file=sys.stderr)
        return 2
    today = datetime.date.today().isoformat()
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        report = list(pool.map(lambda s: check_entry(s, not args.no_registry), servers))

    verified = {r["id"] for r in report if r["state"] == "verified"}
    new_text = stamp(text, verified, today)
    dates = set(re.findall(r'^last_verified = "([0-9-]+)"$', new_text, flags=re.M))
    try:
        new_text = replace_meta(new_text, meta_block(report, dates, today))
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    for state in ("dead", "deprecated", "unknown"):
        rows = [r for r in report if r["state"] == state]
        for r in rows:
            why = "; ".join(f"{p['kind']} {p['target']}: {p['detail']}"
                            for p in r["probes"] if p["state"] != ALIVE)
            print(f"{state.upper():<10} {r['id']:<22} {why}")
    counts = " · ".join(f"{s} {sum(1 for r in report if r['state'] == s)}"
                        for s in ("verified", "dead", "deprecated", "unknown"))
    listed = sum(1 for r in report if r["mcp_registry"])
    print(f"{len(report)} entries · {counts} · listed in the MCP Registry {listed}")

    if args.json:
        pathlib.Path(args.json).write_text(json.dumps(
            {"as_of": today, "source": SOURCES, "entries": report}, indent=2) + "\n")
    if not args.check:
        if new_text != text:
            TARGET.write_text(new_text)
            print(f"wrote {TARGET.relative_to(ROOT)} · {len(verified)} entries dated {today}")
        else:
            print("no change")
    bad = any(r["state"] in ("dead", "deprecated") for r in report)
    return 4 if bad else 0


def selftest():
    sample = "\n".join([
        "x = 1", "", META_BEGIN, "[meta]", "old = 1", META_END, "",
        "[[servers]]", 'id = "a"', 'last_verified = "2026-04-14"', "",
        "[[servers]]", 'id = "b"', 'last_verified = "2026-04-14"', "",
    ])
    stamped = stamp(sample, {"a"}, "2026-11-24")
    assert 'id = "a"\nlast_verified = "2026-11-24"' in stamped, stamped
    assert 'id = "b"\nlast_verified = "2026-04-14"' in stamped, "an unverified entry keeps its date"
    assert classify([("npm", "x", ALIVE, ""), ("remote", "y", ALIVE, "")]) == "verified"
    assert classify([("npm", "x", ALIVE, ""), ("remote", "y", UNKNOWN, "")]) == "unknown"
    assert classify([("npm", "x", DEPRECATED, ""), ("pypi", "y", UNKNOWN, "")]) == "deprecated"
    assert classify([("npm", "x", GONE, ""), ("pypi", "y", DEPRECATED, "")]) == "dead"
    assert classify([]) == "unknown", "an entry with nothing to probe is never verified"
    report = [{"id": "a", "state": "verified", "probes": [], "mcp_registry": "io.x/a"},
              {"id": "b", "state": "dead", "probes": [], "mcp_registry": None}]
    replaced = replace_meta(stamped, meta_block(report, {"2026-11-24", "2026-04-14"}, "2026-11-24"))
    parsed = tomllib.loads(replaced)
    assert parsed["meta"]["verified"] == 1 and parsed["meta"]["dead"] == 1, parsed["meta"]
    assert parsed["meta"]["distinct_last_verified_values"] == 2
    assert parsed["meta"]["last_verified_semantics"] == "per-entry"
    assert "old" not in parsed["meta"], "the region is replaced whole"
    try:
        replace_meta("no markers", "x")
    except ValueError:
        pass
    else:
        raise AssertionError("a file without the region is refused")
    print("selftest ok")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="probe and report, write nothing")
    parser.add_argument("--json", metavar="FILE", help="also save the full report as JSON")
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--no-registry", action="store_true",
                        help="skip the MCP Registry cross-check")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
