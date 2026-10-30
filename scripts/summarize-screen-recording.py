"""Summarize independently checked final runs and native retirement trends."""
import argparse
import json
import math
import pathlib
import statistics


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def p95(values):
    return sorted(values)[max(0, math.ceil(len(values) * .95) - 1)]


def summarize(root):
    rows = {}
    for folder in sorted(root.iterdir()):
        if not folder.is_dir() or not (folder / "plan.json").exists():
            continue
        plan = read(folder / "plan.json")
        decoded = read(folder / "decode-report.json")
        retired = [read(path)["handles"] for path in sorted(folder.glob("native-retired-*.json"))]
        assert len(retired) == plan["rounds"], "Missing native retirement evidence"
        owners = [read(path) for path in sorted(folder.glob("retired-*.json"))]
        assert len(owners) == plan["rounds"] and all(not any(x.values()) for x in owners)
        # Per-round asynchronous native cleanup causes isolated handle spikes.
        # Compare warm baseline medians, retain the full series and range, and
        # still reject sustained hardware growth instead of trusting COM leases.
        tail = retired[5:] if len(retired) >= 20 else retired
        span = min(5, len(tail))
        growth = statistics.median(tail[-span:]) - statistics.median(tail[:span])
        bounded = growth <= 32
        samples = [json.loads(line) for line in
                   (folder / "process-metrics.jsonl").read_text(encoding="utf-8").splitlines()]
        ui = [row["ui"] for row in samples if row["ui"]]
        results = decoded["results"]
        native = [read(path) for path in sorted(folder.glob("round-*.json"))]
        hashes = {read(folder / "build.json")["Hash"]}
        rows[folder.name] = {
            "decoded": decoded["passed"], "rounds": plan["rounds"],
            "build_sha256": next(iter(hashes)),
            "timings_ms": decoded["timings_ms"],
            "minimum_content_fps": min(row["content_fps"] for row in results),
            "native_retired_handles": retired,
            "warm_handle_range": max(tail) - min(tail),
            "warm_median_handle_growth": growth, "native_retirement_bounded": bounded,
            "private_peak_mib": max(row["private_bytes"] for row in samples) / 1048576,
            "working_set_peak_mib": max(row["working_set"] for row in samples) / 1048576,
            "cpu_core_equivalents": (samples[-1]["cpu_100ns"] - samples[0]["cpu_100ns"]) /
                                    10000 / (samples[-1]["ms"] - samples[0]["ms"]),
            "ui_timeouts": sum(not row["delivered"] for row in ui),
            "ui_p95_ms": p95([row["ms"] for row in ui]),
            "controlled_pixel_bytes": max(row["controlled_pixel_bytes"] for row in native),
            "source_buffers_peak": max(row["max_source_buffers"] for row in native),
            "returned_source_buffers": sorted({row["returned_source_buffers"] for row in native}),
        }
        if folder.name == "software-soak":
            rows[folder.name]["private_first_last_minute_median_mib"] = [
                statistics.median(row["private_bytes"] for row in samples if start < row["ms"] < end) / 1048576
                for start, end in [(10000, 60000), (540000, 590000)]]
    expected = {"software-125", "software-100", "software-soak", "input-125", "input-100",
                "nvidia-125", "nvidia-100"}
    assert set(rows) == expected, "Incomplete final matrix"
    assert len({row["build_sha256"] for row in rows.values()}) == 1, "Mixed final builds"
    mandatory = [row for name, row in rows.items() if not name.startswith("nvidia-")]
    hardware = [row for name, row in rows.items() if name.startswith("nvidia-")]
    return {"software_gate_passed": all(row["decoded"] and row["native_retirement_bounded"] for row in mandatory),
            "hardware_qualified": all(row["decoded"] and row["native_retirement_bounded"] for row in hardware),
            "native_handle_rule": "After five warmup runs, compare medians of first/last five retired samples; growth <=32 handles. Raw series and range retained; not a claim of zero driver allocation.",
            "results": rows}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=pathlib.Path)
    args = parser.parse_args()
    try:
        report = summarize(args.root)
    except Exception as error:
        report = {"software_gate_passed": False, "error": str(error)}
    (args.root / "qualification.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))
    raise SystemExit(0 if report["software_gate_passed"] else 1)
