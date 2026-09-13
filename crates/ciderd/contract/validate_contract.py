#!/usr/bin/env python3
"""Validate the design bundle and its synthetic fixtures, not macOS acquisition.

Requires Python 3.11+ and jsonschema. This is a reference fixture checker, not a
production ingest service. Run: python validate_contract.py [bundle-directory]
"""
from __future__ import annotations

import copy
import json
import math
import re
import sys
import plistlib
import tomllib
from datetime import datetime
from pathlib import Path
from typing import Any

try:
    from jsonschema import Draft202012Validator, FormatChecker
except ImportError as exc:
    raise SystemExit("Install the 'jsonschema' Python package to run this validator.") from exc

# Deliberately minimal test values. Production adapters need OS/version-specific
# operation/cache/error dictionaries and an installed-collector allowlist.
FIXTURE_ALLOWLISTS = {
    ("storage.nfs.client.operations_total", "operation"): {"READ"},
}


def load(path: Path) -> Any:
    def reject_nonfinite(value: str) -> None:
        raise ValueError(f"Non-finite JSON numeric value: {value}")
    def unique_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"Duplicate JSON object key: {key}")
            result[key] = value
        return result
    return json.loads(path.read_text(encoding="utf-8"),
                      parse_constant=reject_nonfinite, object_pairs_hook=unique_keys)


def finite_values(value: Any) -> None:
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("Non-finite numeric value")
    if isinstance(value, dict):
        for child in value.values():
            finite_values(child)
    elif isinstance(value, list):
        for child in value:
            finite_values(child)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def utc(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def base_semantic_check(batch: dict[str, Any], catalogue: dict[str, dict[str, Any]],
                   known_resources: dict[str, dict[str, Any]]) -> None:
    """Check only the semantics exercised by this bundle; fail closed on extras."""
    finite_values(batch)
    resources = dict(known_resources)
    local_ids: set[str] = set()
    for resource in batch["resources"]:
        key = resource["resource_id"]
        require(key not in local_ids, f"Duplicate resource ID: {key}")
        local_ids.add(key)
        resources[key] = resource

    def known(resource_id: str) -> dict[str, Any]:
        require(resource_id in resources, f"Unresolved resource: {resource_id}")
        require(resource_id.startswith(batch["node_id"] + "/"),
                f"Resource outside fixture node namespace: {resource_id}")
        return resources[resource_id]

    for relationship in batch["relationships"]:
        known(relationship["from_resource_id"])
        known(relationship["to_resource_id"])
    collection_ids: set[str] = set()
    for collection in batch["collections"]:
        cid = collection["collection_id"]
        require(cid not in collection_ids, f"Duplicate collection ID: {cid}")
        collection_ids.add(cid)
        resource = known(collection["resource_id"])
        require(int(collection["finished_monotonic_ns"]) >= int(collection["started_monotonic_ns"]),
                f"Negative monotonic acquisition duration: {cid}")
        # Wall-clock reversal is intentionally not rejected: clock correction can
        # happen during a collection. Monotonic bounds are authoritative.
        utc(collection["started_at"])
        utc(collection["finished_at"])
        series: set[tuple[str, str]] = set()
        for metric in collection["metrics"]:
            name = metric["name"]
            require(name in catalogue, f"Unknown metric: {name}")
            definition = catalogue[name]
            require(metric["kind"] == definition["kind"], f"Wrong kind: {name}")
            require(metric["unit"] == definition["unit"], f"Wrong unit: {name}")
            require(resource["resource_type"] in definition["scope"], f"Wrong scope: {name}")
            require(set(metric["attributes"]) == set(definition["attributes"]),
                    f"Wrong attribute keys: {name}")
            for key, value in metric["attributes"].items():
                rule = definition["attributes"][key]
                allowlist = set(rule) if isinstance(rule, list) else FIXTURE_ALLOWLISTS.get((name, key))
                require(allowlist is not None, f"No fixture allowlist for {name}.{key}")
                require(value in allowlist, f"Attribute outside allowlist: {name}.{key}")
            series_id = (name, json.dumps(metric["attributes"], sort_keys=True))
            require(series_id not in series, f"Duplicate series in one collection: {name}")
            series.add(series_id)
            if metric["availability"] == "available":
                require(metric["value_type"] == definition["value_type"], f"Wrong value type: {name}")
                if metric["value_type"] == "integer":
                    require(str(int(metric["value"])) == metric["value"], f"Noncanonical integer: {name}")
    for event in batch["events"]:
        known(event["resource_id"])



def semantic_check(batch, catalogue, known_resources):
    base_semantic_check(batch, catalogue, known_resources)
    require(int(batch["agent_generation"]) <= 2**64-1, "Agent generation exceeds u64")
    require(int(batch["sequence"]) <= 2**64-1, "Sequence exceeds u64")
    collections = {c["collection_id"]: c for c in batch["collections"]}
    resources = {**known_resources, **{r["resource_id"]: r for r in batch["resources"]}}
    states = set()
    for state in batch["collector_states"]:
        key = (state["collector"], state["resource_id"])
        require(key not in states, "Duplicate collector scope")
        states.add(key)
        require(state["resource_id"] in resources, "Unknown collector-state resource")
        if "last_attempt_id" in state:
            require(state["last_attempt_id"] in collections, "Unresolved last_attempt_id")
            c = collections[state["last_attempt_id"]]
            require((c["collector"], c["resource_id"]) == key, "last_attempt_id scope mismatch")
        if state["phase"] == "timed_out_pending_exit":
            require("last_attempt_id" in state, "Quarantined worker lacks timeout attempt")
            require(collections[state["last_attempt_id"]]["status"] == "timeout", "Quarantine without timeout")
    for c in collections.values():
        if c["clock_id"] == batch["clock_id"]:
            require(int(c["finished_monotonic_ns"]) <= int(batch["monotonic_ns"]),
                    "Sample occurs after heartbeat on the same clock")
        for metric in c["metrics"]:
            if metric.get("value_type") == "integer" and metric["availability"] == "available":
                value = int(metric["value"])
                if metric["kind"] == "counter":
                    require(0 <= value <= 2**128-1, "Counter exceeds u128")
                else:
                    require(-(2**127) <= value <= 2**128-1, "Integer gauge exceeds supported range")


def check_ack(request, ack, validator):
    validator.validate(ack)
    require(ack["agent_session_id"] == request["agent_session_id"], "Wrong acknowledgement session")
    require(ack["accepted_sequence"] == request["sequence"], "Wrong acknowledgement sequence")
    if not ack["request_inventory"]:
        require(ack["inventory_revision"] == request["inventory"]["revision"],
                "Wrong acknowledged inventory revision")


def verify_immutable_collections(first, second):
    known = {c["collection_id"]: c for c in first["collections"]}
    for c in second["collections"]:
        if c["collection_id"] in known:
            require(c == known[c["collection_id"]], "Repeated collection ID changed content")


def advances_live(current, candidate):
    """Illustrative session fencing, not an ingestion service."""
    old_gen, new_gen = int(current["agent_generation"]), int(candidate["agent_generation"])
    if new_gen != old_gen:
        return new_gen > old_gen
    return (candidate["agent_session_id"] == current["agent_session_id"]
            and int(candidate["sequence"]) > int(current["sequence"]))


def main(directory: Path) -> None:
    schema = load(directory / "heartbeat.schema.json")
    ack_schema = load(directory / "heartbeat-ack.schema.json")
    for s in (schema, ack_schema):
        Draft202012Validator.check_schema(s)
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    ack_validator = Draft202012Validator(ack_schema, format_checker=FormatChecker())
    metrics = load(directory / "metric-catalog.json")["metrics"]
    catalogue = {m["name"]: m for m in metrics}
    require(len(catalogue) == len(metrics), "Duplicate metric names")
    print(f"PASS: heartbeat and acknowledgement schema meta-validation; {len(catalogue)} unique metric definitions")
    startup = load(directory / "example-startup-heartbeat.json")
    resources = {r["resource_id"]: r for r in startup["resources"]}
    fixtures = {}
    for name in ("example-startup-heartbeat.json", "example-heartbeat.json",
                 "example-degraded-heartbeat.json", "example-repeated-heartbeat.json"):
        data = load(directory / name)
        validator.validate(data)
        semantic_check(data, catalogue, resources)
        fixtures[name] = data
        print(f"PASS: {name}: schema, references, metric semantics, scope and timing")
    normal = fixtures["example-heartbeat.json"]
    repeated = fixtures["example-repeated-heartbeat.json"]
    degraded = fixtures["example-degraded-heartbeat.json"]
    verify_immutable_collections(normal, repeated)
    verify_immutable_collections(normal, degraded)
    require(normal["sequence"] != repeated["sequence"], "Repeat must have new heartbeat sequence")
    require(normal["collections"] == repeated["collections"], "Repeat fixture must preserve all observations")
    print("PASS: new heartbeat sequence repeats identical original collection IDs/times/values")
    ack = load(directory / "example-heartbeat-ack.json")
    check_ack(normal, ack, ack_validator)
    print("PASS: acknowledgement matches session, sequence and inventory revision")
    org = (directory / "guide.org").read_text()
    blocks = re.findall(r"#\+begin_src json\s*\n(.*?)#\+end_src", org, re.I|re.S)
    for text in blocks:
        data = json.loads(text)
        validator.validate(data)
        semantic_check(data, catalogue, resources)
    print(f"PASS: {len(blocks)} inline JSON heartbeat example(s)")
    huge = normal["collections"][0]["metrics"][0]["value"]
    require(int(huge) > 2**53, "Precision fixture is too small")
    require(json.loads(json.dumps(normal))["collections"][0]["metrics"][0]["value"] == huge,
            "Large integer changed in round trip")
    require(int(huge) - int(str(int(huge)-17)) == 17, "Exact subtraction failed")
    print("PASS: wide counter round-trip and exact integer delta")

    count = 0
    def rejects(label, fn):
        nonlocal count
        from jsonschema.exceptions import ValidationError
        try:
            fn()
        except (ValueError, ValidationError):
            count += 1
            print(f"PASS: rejects {label}")
        else:
            raise ValueError(f"Invalid fixture accepted: {label}")
    def check(data):
        validator.validate(data)
        semantic_check(data, catalogue, resources)
    def variant():
        d = copy.deepcopy(normal)
        return d, d["collections"][0]["metrics"][0]
    d,m=variant(); m["value"]=123
    rejects("numeric rather than decimal-string counter", lambda:check(d))
    d,m=variant(); m.pop("counter_epoch")
    rejects("counter missing epoch", lambda:check(d))
    d,m=variant(); m["value"]="-1"
    rejects("negative counter", lambda:check(d))
    d,m=variant(); m["value"]="001"
    rejects("noncanonical decimal string", lambda:check(d))
    d,m=variant(); m.pop("value")
    rejects("available value missing", lambda:check(d))
    d=copy.deepcopy(degraded); d["collections"][0]["metrics"][0]["value"]="0"
    rejects("fabricated zero on timeout", lambda:check(d))
    d,m=variant(); m["unit"]="seconds"
    rejects("unit mismatch", lambda:check(d))
    d,m=variant(); d["collections"][0]["resource_id"]="node-example-01/mount-nfs-a"
    rejects("device I/O attributed to NFS mount", lambda:check(d))
    d,m=variant(); d["collections"][0]["finished_monotonic_ns"]="1"
    rejects("reversed monotonic bounds", lambda:check(d))
    d,m=variant(); m["attributes"]={"filename":"/unbounded/path"}
    rejects("high-cardinality attribute", lambda:check(d))
    d,m=variant(); m["value"]=str(2**128)
    rejects("u128 overflow", lambda:check(d))
    d=copy.deepcopy(normal); d["monotonic_ns"]="1"
    rejects("sample later than heartbeat on same clock", lambda:check(d))
    d=copy.deepcopy(normal); d["collector_states"][0]["last_attempt_id"]="missing"
    rejects("unresolved last-attempt reference", lambda:check(d))
    d=copy.deepcopy(normal); d["collector_states"].append(copy.deepcopy(d["collector_states"][0]))
    rejects("duplicate collector scope", lambda:check(d))
    d=copy.deepcopy(normal); d["inventory"]["included"]=False
    rejects("hidden inventory records with included=false", lambda:check(d))
    d=copy.deepcopy(normal); d["agent_generation"]=str(2**64)
    rejects("agent-generation overflow", lambda:check(d))
    a=copy.deepcopy(ack); a["agent_session_id"]="another-session"
    rejects("acknowledgement for another session", lambda:check_ack(normal,a,ack_validator))
    a=copy.deepcopy(ack); a["accepted_sequence"]="0"
    rejects("acknowledgement for another sequence", lambda:check_ack(normal,a,ack_validator))
    a=copy.deepcopy(ack); a["inventory_revision"]="1"
    rejects("wrong acknowledged inventory revision", lambda:check_ack(normal,a,ack_validator))
    a=copy.deepcopy(ack); a["command"]="arbitrary-shell-command"
    rejects("remote command embedded in acknowledgement", lambda:check_ack(normal,a,ack_validator))
    d=copy.deepcopy(repeated); d["collections"][0]["finished_at"]=d["created_at"]
    rejects("restamped collection reused under its original ID", lambda:verify_immutable_collections(normal,d))
    print(f"PASS: {count} invalid-input cases rejected")
    require(advances_live(normal,repeated), "Fresh sequence should advance live state")
    require(not advances_live(normal,normal), "Duplicate must not advance live state")
    old=copy.deepcopy(repeated); old["agent_generation"]="6"; old["sequence"]="999999"
    require(not advances_live(normal,old), "Older generation must not advance live state")
    require(not advances_live(repeated,normal), "Out-of-order sequence must not advance live state")
    print("PASS: duplicate/out-of-order/retired-generation liveness fencing")
    cfg=tomllib.loads((directory.parent/"examples"/"ciderd.toml").read_text())
    require(cfg["heartbeat"]["endpoint"].startswith("https://"),"HTTPS required")
    require(cfg["heartbeat"]["interval_seconds"] == 5, "Fixture heartbeat cadence must remain five seconds")
    require(0 < cfg["heartbeat"]["connect_timeout_seconds"] <= cfg["heartbeat"]["request_timeout_seconds"] <= 300,
            "Fixture connection/request deadlines must match runtime bounds")
    require(cfg["heartbeat"]["delivery_mode"]=="latest", "Latest-state fixture required")
    require(not cfg["collection"]["active_write_probes_enabled"], "Active writes must be opt-in")
    launch=plistlib.loads((directory.parent/"examples"/"org.example.ciderd.plist").read_bytes())
    require(launch["ProgramArguments"][0].startswith("/"), "Absolute daemon path required")
    require(launch["ProgramArguments"][1]=="run", "Foreground run mode required")
    require(launch["UserName"]=="root", "Root fixture required")
    require(launch["WorkingDirectory"]=="/", "Safe working directory required")
    require(launch["Umask"]==0o77, "Owner-only file creation fixture required")
    print("PASS: TOML and launchd plist parse; expected timing, foreground and safety settings")
    refs=set(re.findall(r"\[fn:([\w-]+)\]", org))
    defs=set(re.findall(r"^\[fn:([\w-]+)\]", org,re.M))
    require(refs <= defs,"Undefined Org source footnotes")
    require(org.lower().count("#+begin_src")==org.lower().count("#+end_src"),"Unbalanced Org source blocks")
    print(f"PASS: Org block structure and {len(defs)} source-footnote definitions")
    print("BOUNDARY: static contract/configuration/fixture checks only; no Rust compilation, macOS acquisition or launchd deployment executed.")


if __name__ == "__main__":
    if len(sys.argv)>2:
        raise SystemExit("Usage: validate_contract.py [bundle-directory]")
    main(Path(sys.argv[1]) if len(sys.argv)==2 else Path(__file__).resolve().parent)
