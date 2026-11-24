#!/usr/bin/env bash
# Refresh data/model-pricing.toml from models.dev (MIT · sst/models.dev).
#
# The pricing catalog is a VENDORED SNAPSHOT, refreshed at maintenance
# time — never a runtime fetch (supernovae-alignment Rule 1: a static
# report must not depend on a vendor endpoint being up). models.dev is
# the one machine-readable source covering every cloud provider in the
# engine catalog (verified 2026-07-06: openrouter 336 priced models ·
# huggingface 50 · nvidia 84 — LiteLLM carries zero huggingface).
#
# Usage:
#   bash scripts/refresh-pricing.sh              # fetch live
#   bash scripts/refresh-pricing.sh api.json     # from a local snapshot
#
# Emission laws (matched to src/data/models.rs lookup semantics):
# - provider strings = the ENGINE's canonical ids (anthropic · google ·
#   …) so `find_pricing_scoped(engine_id, …)` matches (the hand era
#   wrote display names — "Gemini" — which eq_ignore_ascii_case never
#   matched against "google").
# - within a provider, rules sort by pattern length DESC: the contains()
#   fallback takes the FIRST hit, so `claude-opus-4-5` must outrank any
#   shorter pattern a versioned id also contains.
# - values always carry a decimal point (TOML integers do not
#   deserialize into f64 fields). This applies to the RATES only — the
#   token limits are u32 and must stay integers.
# - a field upstream does not carry is OMITTED, never defaulted. Absent
#   means "not disclosed"; a zero would mean "bounded by nothing", which
#   is the opposite claim. Upstream writes 0 for models where a limit
#   does not apply (image · video · speech) — those are dropped too.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="${1:-}"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
if [ -z "$SRC" ]; then
  curl -sL --max-time 120 https://models.dev/api.json -o "$TMP"
  SRC="$TMP"
fi

python3 - "$SRC" <<'PYEOF'
import hashlib
import json
import sys
import tomllib
from datetime import date

src = sys.argv[1]
raw = open(src, "rb").read()
sha = hashlib.sha256(raw).hexdigest()[:16]
data = json.loads(raw)

# ── The research merge step ─────────────────────────────────────────
# @1.3 research facts (energy · tokenizer · determinism) are vendored by
# the research lane with per-claim sources; upstream does not carry them.
# This refresh rewrites the whole file, so before regenerating it HARVESTS
# those facts from the live TOML by (provider, model_pattern) and
# re-attaches them to the rows that survive. A fact whose row vanished
# upstream is reported LOUDLY — a named loss for the diff review, never a
# silent wipe (the ONE DOOR pipeline will absorb this leg · wave-1.5 §3bis).
RESEARCH_FIELDS = ("energy", "tokenizer", "determinism")
research = {}
target = "crates/nika-catalog/data/model-pricing.toml"
try:
    live = tomllib.load(open(target, "rb"))
except OSError:
    live = {"rules": []}
for row in live.get("rules", []):
    kept = {f: row[f] for f in RESEARCH_FIELDS if f in row}
    if kept:
        research[(row["provider"], row["model_pattern"])] = kept

# Engine canonical id → models.dev provider id (identical today; the
# map exists so a future rename upstream is one line here).
PROVIDERS = {
    "anthropic": "anthropic",
    "openai": "openai",
    "google": "google",
    "deepseek": "deepseek",
    "mistral": "mistral",
    "xai": "xai",
    "groq": "groq",
    "openrouter": "openrouter",
    "huggingface": "huggingface",
    "nvidia": "nvidia",
}

def toml_float(v: float) -> str:
    s = f"{v:.10g}"
    return s if ("." in s or "e" in s) else s + ".0"

def limit(m: dict, key: str):
    """A token limit, or None when upstream is silent OR says 0.

    models.dev writes 0 for models where the limit is not a thing —
    image, video and speech models. Carrying that 0 through would claim
    a ceiling of nothing, so 0 and absent collapse to the same answer:
    we do not know. The schema stores that as an omitted field."""
    v = (m.get("limit") or {}).get(key)
    if not isinstance(v, int) or v <= 0:
        return None
    return v

def status_of(m: dict):
    """Upstream lifecycle marker (deprecated · beta · whatever they add
    next). Shape-checked only — the vocabulary is theirs to grow, and a
    refresh must not fail because they coined a new word."""
    s = m.get("status")
    if not isinstance(s, str):
        return None
    s = s.strip().lower()
    # Byte rule kept identical to validate_status() in the codegen crate:
    # [a-z0-9-]{1,32}. Anything else is upstream garbage, not a status.
    ok = s and len(s) <= 32 and all(
        ("a" <= c <= "z") or ("0" <= c <= "9") or c == "-" for c in s
    )
    return s if ok else None

# Referenced-but-retired: models the PROVIDER CATALOG still points at
# (default_model rows) that upstream dropped — priced from LiteLLM's
# historical table (values verified 2026-07-06). The integrity gate
# (validate::provider_models_have_pricing) holds through upstream churn;
# retiring these requires the default_model canon to move first. The xAI
# grok-3 supplements left when the defaults moved to grok-4.x on 2026-10-12.
#
# Served-but-unlisted: the vendor aliases DeepSeek's chat/completions
# still answers although its /models list and pricing page dropped them
# (verified 2026-09-23 · one token each · both answer `"model":
# "deepseek-flash"`). Priced as the model they resolve to — the
# deepseek-flash row above — so a human naming the alias they know is
# bounded at the run door instead of refused NIKA-1709 as unpriced.
# DeepSeek retired both names (API change log, 2026-04-24: discontinued
# 2026-07-24), so their rows keep the `deprecated` status models.dev gave
# them while it listed them: priced for a recorded run, never offered.
#
# (provider, model, input, output, status)
SUPPLEMENTS = [
    ("anthropic", "claude-sonnet-4", 3.0, 15.0, None),
    ("deepseek", "deepseek-chat", 0.15, 0.6, "deprecated"),
    ("deepseek", "deepseek-reasoner", 0.15, 0.6, "deprecated"),
]

# Corrections: an upstream row a vendor's own page contradicts, set to
# the page's figure on every refresh, with the page and the reason
# emitted above the row. A correction upstream already matches is
# reported for removal; one whose row vanished is reported loudly. A
# value of None removes the field (the page shows no such price).
#
# Gemini 4 Carbon and Argon (model pages, read 2026-11-24): a 65,536-token
# synchronous output limit, and input, output and cache-read prices of
# $2.00 / $10.00 / $0.10 through 2026-11-30, then $4.00 / $20.00 / $0.20
# from 2026-12-01. The later prices are carried, so these rows over-state
# the cost until 2026-11-30 rather than under-state it afterwards.
# Mistral Small 4 (docs.mistral.ai/inference/pricing, read 2026-11-24): the
# cached-input cell is empty, so no cache-read price is claimed.
#
# (provider, model, {field: value}, source, reason)
GEMINI_4_DEC_1 = {
    "max_output_tokens": 65536,
    "input": 4.0,
    "output": 20.0,
    "cache_read": 0.2,
}
CORRECTIONS = [
    ("google", "gemini-4-carbon", GEMINI_4_DEC_1,
     "https://ai.google.dev/gemini-api/docs/models/gemini-4-carbon.md.txt",
     "65,536 output tokens; the 2026-12-01 prices, which over-state the cost until 2026-11-30"),
    ("google", "gemini-4-argon", GEMINI_4_DEC_1,
     "https://ai.google.dev/gemini-api/docs/models/gemini-4-argon.md.txt",
     "65,536 output tokens; the 2026-12-01 prices, which over-state the cost until 2026-11-30"),
    ("mistral", "mistral-small-latest", {"cache_read": None},
     "https://docs.mistral.ai/inference/pricing",
     "no cached-input price on the pricing page"),
    ("mistral", "mistral-small-2603", {"cache_read": None},
     "https://docs.mistral.ai/inference/pricing",
     "no cached-input price on the pricing page"),
]

rules = []
seen = set()
for engine_id, upstream_id in PROVIDERS.items():
    prov = data.get(upstream_id)
    if not prov:
        print(f"WARN: {upstream_id} missing upstream", file=sys.stderr)
        continue
    for mid, m in prov["models"].items():
        cost = m.get("cost")
        if not cost or "input" not in cost or "output" not in cost:
            continue  # unpriced (previews · embeddings without rates) — absent ≠ free
        key = (engine_id, mid)
        if key in seen:
            continue
        seen.add(key)
        # Cache axes when upstream discloses them (2026-07: cache_read on
        # 2209 models · cache_write on 756). The cost math prices the
        # cache subsets at these rates; without them it falls back to the
        # input rate (documented approximation).
        cr = cost.get("cache_read")
        cw = cost.get("cache_write")
        ow = m.get("open_weights")
        rules.append({
            "provider": engine_id,
            "pattern": mid,
            "input": float(cost["input"]),
            "output": float(cost["output"]),
            "cache_read": None if cr is None else float(cr),
            "cache_write": None if cw is None else float(cw),
            # Upstream model facts (schema @1.2) — carried verbatim, or
            # omitted. Never defaulted: see the emission laws above.
            "max_output_tokens": limit(m, "output"),
            "context_window_tokens": limit(m, "context"),
            "open_weights": ow if isinstance(ow, bool) else None,
            "status": status_of(m),
        })

# Supplements are hand-priced from LiteLLM's historical table and carry
# NO model facts — upstream dropped these models, so there is nothing to
# mirror. Every @1.2 field stays absent rather than guessed, except the
# retirement status its vendor published.
for prov, mid, inp, outp, status in SUPPLEMENTS:
    if (prov, mid) not in seen:
        seen.add((prov, mid))
        rules.append({
            "provider": prov, "pattern": mid, "input": inp, "output": outp,
            "cache_read": None, "cache_write": None,
            "max_output_tokens": None, "context_window_tokens": None,
            "open_weights": None, "status": status,
        })

for prov, mid, fields, source, reason in CORRECTIONS:
    row = next((r for r in rules if (r["provider"], r["pattern"]) == (prov, mid)), None)
    if row is None:
        print(f"ORPHAN correction, its row is gone upstream: {prov}/{mid}", file=sys.stderr)
        continue
    if all(row.get(f) == v for f, v in fields.items()):
        print(f"NOTE correction now matches upstream, drop it: {prov}/{mid}", file=sys.stderr)
    row.update(fields)
    row["corrected"] = f"# corrected from {source}: {reason}"

# Fallback guard: a row upstream dropped whose id contains a pattern still listed for its
# provider would be priced through the contains() fallback as that other row (o3-mini as o3,
# gpt-4.1-nano as gpt-4.1), a silent wrong cost. Such a row is carried forward from the live
# file with its last prices and a `# kept from` line naming the row it would fall into; a
# dropped row that falls into nothing is dropped (a call to it is refused as unpriced).
live_rows = live.get("rules", [])
listed = {(r["provider"], r["pattern"]) for r in rules}
for old in live_rows:
    prov, pat = old["provider"], old["model_pattern"]
    if (prov, pat) in listed:
        continue
    into = sorted((r["pattern"] for r in rules
                   if r["provider"] == prov and r["pattern"] != pat and r["pattern"] in pat),
                  key=len, reverse=True)
    if not into:
        continue
    listed.add((prov, pat))
    rules.append({
        "provider": prov, "pattern": pat,
        "input": old["input_per_million"], "output": old["output_per_million"],
        "cache_read": old.get("cache_read_per_million"),
        "cache_write": old.get("cache_write_per_million"),
        "max_output_tokens": old.get("max_output_tokens"),
        "context_window_tokens": old.get("context_window_tokens"),
        "open_weights": old.get("open_weights"), "status": old.get("status"),
        "kept": (f"# kept from the previous snapshot: models.dev no longer lists it, and "
                 f"without this row it would be priced as `{into[0]}`"),
    })
    print(f"KEPT {prov}/{pat}: it would fall into {into[0]}", file=sys.stderr)

# Contains-fallback safety: longest pattern first within each provider.
rules.sort(key=lambda r: (r["provider"], -len(r["pattern"]), r["pattern"]))

# Shrink guard (the LiteLLM pattern): a refresh that loses more than half
# the previous rules is a corrupted/partial upstream, not a real catalog
# move — refuse instead of silently shipping a gutted snapshot.
try:
    prev = open(target).read().count("[[rules]]")
except OSError:
    prev = 0
if prev and len(rules) < prev // 2:
    print(
        f"ERROR: refreshed catalog shrank {prev} → {len(rules)} rules "
        f"(more than half) — corrupted upstream? refusing to write. "
        f"Set NIKA_PRICING_FORCE=1 to override.",
        file=sys.stderr,
    )
    import os
    if not os.environ.get("NIKA_PRICING_FORCE"):
        sys.exit(1)

out = []
out.append('schema = "nika/model-pricing@1.3"')
out.append("")
out.append("[meta]")
out.append('source = "https://models.dev/api.json"')
out.append(f'as_of = "{date.today().isoformat()}"')
out.append(f'source_sha256_16 = "{sha}"')
out.append("")
out.append("# ── GENERATED · do not hand-edit ────────────────────────────")
out.append("# Source: models.dev (MIT · anomalyco/models.dev) — see [meta].")
out.append("# Refresh: bash scripts/refresh-pricing.sh")
out.append("# Values: USD per 1M tokens. Lookup: exact match, then contains()")
out.append("# fallback (first hit wins — longest patterns emitted first).")
out.append("# Cache axes ride along when upstream discloses them; the cost")
out.append("# math prices cache subsets at these rates (input-rate fallback).")
out.append("# Token limits · open_weights · status are upstream's own words,")
out.append("# carried verbatim. An OMITTED field means upstream did not say —")
out.append("# read it as unknown, never as 0 / false.")
out.append("# Research facts (@1.3 · energy · tokenizer · determinism) are")
out.append("# vendored by the research lane with per-claim sources — this")
out.append("# refresh never AUTHORS them, it re-attaches the live file's rows")
out.append("# by (provider, model_pattern) and names any orphan loudly. An")
out.append("# OMITTED research field means no sourced fact exists yet.")
out.append("# A row a vendor page contradicts carries that page's figure and a")
out.append("# `# corrected from` line naming it (CORRECTIONS in the refresh).")
out.append("# A row upstream dropped that the contains() fallback would price as")
out.append("# another row keeps its last prices under a `# kept from` line.")

current = None
for r in rules:
    prov = r["provider"]
    if prov != current:
        n = sum(1 for x in rules if x["provider"] == prov)
        out.append("")
        out.append(f"# ── {prov} ({n} models) ──")
        current = prov
    out.append("")
    if r.get("corrected"):
        out.append(r["corrected"])
    if r.get("kept"):
        out.append(r["kept"])
    out.append("[[rules]]")
    out.append(f'provider = "{prov}"')
    out.append(f'model_pattern = "{r["pattern"]}"')
    out.append(f"input_per_million = {toml_float(r['input'])}")
    out.append(f"output_per_million = {toml_float(r['output'])}")
    if r["cache_read"] is not None:
        out.append(f"cache_read_per_million = {toml_float(r['cache_read'])}")
    if r["cache_write"] is not None:
        out.append(f"cache_write_per_million = {toml_float(r['cache_write'])}")
    # u32 fields — emitted as TOML integers (no decimal point).
    if r["max_output_tokens"] is not None:
        out.append(f"max_output_tokens = {r['max_output_tokens']}")
    if r["context_window_tokens"] is not None:
        out.append(f"context_window_tokens = {r['context_window_tokens']}")
    if r["open_weights"] is not None:
        out.append(f"open_weights = {str(r['open_weights']).lower()}")
    if r["status"] is not None:
        out.append(f'status = "{r["status"]}"')
    kept = research.pop((prov, r["pattern"]), None)
    if kept:
        if "tokenizer" in kept:
            out.append(f"tokenizer = {json.dumps(kept['tokenizer'])}")
        if "determinism" in kept:
            out.append(f"determinism = {json.dumps(kept['determinism'])}")
        if "energy" in kept:
            e = kept["energy"]
            out.append(
                "energy = { "
                f"wh_per_mtok_out = {toml_float(e['wh_per_mtok_out'])}, "
                f"provenance = {json.dumps(e['provenance'])}, "
                f"scope = {json.dumps(e['scope'])}, "
                f"source = {json.dumps(e['source'])}, "
                f"measured_at = {json.dumps(e['measured_at'])}"
                " }"
            )

# Orphans: research facts whose row upstream dropped. Named, never
# silent — the diff review decides whether to re-home or accept the loss.
for (prov, pat), kept in sorted(research.items()):
    print(
        f"ORPHAN research fact dropped with its row: {prov}/{pat} "
        f"({' · '.join(sorted(kept))}) — re-vendor or accept the loss",
        file=sys.stderr,
    )

open(target, "w").write("\n".join(out) + "\n")
per = {}
for r in rules:
    per[r["provider"]] = per.get(r["provider"], 0) + 1
def n_with(field):
    return sum(1 for r in rules if r[field] is not None)
print(
    f"wrote {target}: {len(rules)} rules · "
    + " · ".join(f"{k}:{v}" for k, v in sorted(per.items()))
)
# Coverage per field — the honest report of what upstream disclosed. A
# number well below the rule count is upstream being silent, not a bug.
print(
    "  facts: "
    f"cache_read {n_with('cache_read')} · "
    f"max_output_tokens {n_with('max_output_tokens')} · "
    f"context_window_tokens {n_with('context_window_tokens')} · "
    f"open_weights {n_with('open_weights')} · "
    f"status {n_with('status')}"
    f"  (of {len(rules)})"
)
reattached = sum(
    1 for line in out if line.startswith(("tokenizer = ", "determinism = ", "energy = "))
)
print(f"  research: {reattached} fact line(s) re-attached · {len(research)} orphan(s)")
PYEOF
