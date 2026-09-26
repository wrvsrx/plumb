#!/usr/bin/env python3
"""Measure release CLI category checks on one deterministic, immutable archive."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import sqlite3
import subprocess
import time


def fixture(events):
    records = []
    for i in range(events):
        title = f"Event {i} `->{{activities.plumb#a{i % 4}}}"
        category = ""
        if i % 1000 == 0:
            title = f"Unclassified {i}"
        elif i % 1000 == 1:
            title = f"Unresolved {i} `->{{activities.plumb#missing}}"
        elif i % 1000 == 2:
            category = " `= event-category\n"
        elif i % 1000 == 3:
            title += " `->{activities.plumb#a0} `->{activities.plumb#a1}"
        records.append(f"`- 2026-09-22T10:00:00Z--11:00 {title}\n `+ event\n{category}")
    return {
        "archive.plumb": "".join(records),
        "activities.plumb": "".join(
            f"`- Activity {i}\n `@ a{i}\n `= event-category\n  `- work\n  `- category {i}\n"
            for i in range(4)
        ),
        "invalid.plumb": "`- {unclosed\n",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--label", choices=["before", "after"], required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--events", type=int, default=34831)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if args.events < 4 or args.samples < 1:
        parser.error("events must be >= 4 and samples >= 1")
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    result_path = work / f"{args.label}.json"
    if result_path.exists():
        parser.error(f"result already exists: {result_path}")
    root = work / "workspace"
    root.mkdir(exist_ok=True)
    sources = fixture(args.events)
    for name, text in sources.items():
        path = root / name
        data = text.encode()
        if path.exists():
            assert path.read_bytes() == data, f"fixture changed: {path}"
        else:
            path.write_bytes(data)
    identity = {name: hashlib.sha256(text.encode()).hexdigest() for name, text in sources.items()}
    rows = []
    started = time.time()

    def run(mode, sample, cache, no_cache=False):
        command = [str(args.binary.resolve()), "check", "--root", str(root),
                   "--config", "diagnostics.event-category.enabled=true", "--cache-stats"]
        if no_cache:
            command.append("--no-cache")
        env = dict(os.environ, PLUMB_CACHE_DIR=str(cache))
        prefix = work / f"{args.label}-{mode}-{sample}"
        with prefix.with_suffix(".stdout").open("wb") as out, prefix.with_suffix(".stderr").open("wb") as err:
            begin = time.perf_counter()
            child = subprocess.Popen(command, env=env, stdout=out, stderr=err)
            _, status, usage = os.wait4(child.pid, 0)
            elapsed = time.perf_counter() - begin
            child.returncode = os.waitstatus_to_exitcode(status)
        row = dict(mode=mode, sample=sample, seconds=elapsed,
                   peak_rss_kib=usage.ru_maxrss, exit_code=child.returncode,
                   stdout_sha256=hashlib.sha256(prefix.with_suffix(".stdout").read_bytes()).hexdigest(),
                   stderr=prefix.with_suffix(".stderr").read_text())
        assert row["exit_code"] == 2, row
        expected_stats = "documents=3 hits=3 parsed=0" if mode == "warm" else "documents=3 hits=0 parsed=3"
        assert expected_stats in row["stderr"], row
        assert "plumb: warning:" not in row["stderr"], row
        output_text = prefix.with_suffix(".stdout").read_text()
        for code in ["event-category.missing", "agenda.invalid-category", "agenda.invalid-item",
                     "syntax.unclosed-inline-group"]:
            assert code in output_text, code
        # The pre-fix memory backend adds "current" to this one diagnostic.
        # Preserve raw hashes; normalize only that known baseline discrepancy.
        canonical = prefix.with_suffix(".stdout").read_bytes()
        if args.label == "before":
            canonical = canonical.replace(b"error[agenda.invalid-document]: document has no current valid semantic output",
                                          b"error[agenda.invalid-document]: document has no valid semantic output")
        row["comparison_sha256"] = hashlib.sha256(canonical).hexdigest()
        if rows:
            assert row["comparison_sha256"] == rows[0]["comparison_sha256"], row
        rows.append(row)
        print(json.dumps({k: v for k, v in row.items() if k != "stderr"}), flush=True)

    # No OS page-cache eviction. Each cold run gets a new application cache.
    for sample in range(args.samples):
        cache = work / f"{args.label}-cold-{sample}"
        assert not cache.exists(), cache
        run("cold", sample, cache)
    warm = work / f"{args.label}-warm"
    assert not warm.exists(), warm
    run("warmup", 0, warm)
    for sample in range(args.samples):
        run("warm", sample, warm)
    for sample in range(args.samples):
        run("no-cache", sample, work / f"{args.label}-unused", True)
    ended = time.time()
    databases = list((work / f"{args.label}-cold-0").rglob("*.sqlite3"))
    assert len(databases) == 1, databases
    with sqlite3.connect(f"file:{databases[0]}?mode=ro", uri=True) as connection:
        assert connection.execute("SELECT count(*) FROM events").fetchone()[0] == args.events
        assert connection.execute("SELECT count(*) FROM documents WHERE valid=0").fetchone()[0] == 1
    result = dict(label=args.label, binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  events=args.events, fixture_sha256=identity, host=platform.node(), platform=platform.platform(),
                  rustc=subprocess.check_output(["rustc", "--version"], text=True).strip(),
                  started_unix=started, duration_seconds=ended-started, midpoint_unix=(started+ended)/2,
                  samples=rows, medians={mode: statistics.median(r["seconds"] for r in rows if r["mode"] == mode)
                                         for mode in ["cold", "warm", "no-cache"]})
    other = work / ("before.json" if args.label == "after" else "after.json")
    if other.exists():
        previous = json.loads(other.read_text())
        assert previous["fixture_sha256"] == identity
        assert previous["samples"][0]["comparison_sha256"] == rows[0]["comparison_sha256"]
    result_path.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result["medians"]), flush=True)


if __name__ == "__main__":
    main()
