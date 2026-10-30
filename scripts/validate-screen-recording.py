"""Independently decode native gate artifacts; never treat container FPS as content FPS."""
import argparse
import hashlib
import json
import math
import re
import pathlib
import os
import statistics
import subprocess
import time
import traceback
from fractions import Fraction


def invoke(args):
    return subprocess.check_output(args, stderr=subprocess.PIPE)


def percentile(values, p):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * p) - 1)]


def work_area(folder, plan):
    """Locate legitimate shell space; never waive the image-content color checks."""
    sidecar=folder/"verification-layout.json"
    if sidecar.exists():
        return json.loads(sidecar.read_text(encoding="utf-8-sig"))["work_area"]
    if os.name != "nt":
        return None
    import ctypes as c
    from ctypes import wintypes as w
    class Rect(c.Structure):
        _fields_=[(key,w.LONG) for key in ["left","top","right","bottom"]]
    class Info(c.Structure):
        _fields_=[("size",w.DWORD),("monitor",Rect),("work",Rect),("flags",w.DWORD)]
    query=list(plan["roi"])
    layout=folder.parent/"layout.json"
    if layout.exists():
        original=json.loads(layout.read_text(encoding="utf-8-sig"))["original"]
        query=[original[0],original[1],original[0]+original[2],original[1]+original[3]]
    api=c.WinDLL("user32")
    api.SetThreadDpiAwarenessContext.argtypes=[w.HANDLE]
    api.SetThreadDpiAwarenessContext.restype=w.HANDLE
    old=api.SetThreadDpiAwarenessContext(w.HANDLE(-4))
    try:
        api.MonitorFromRect.argtypes=[c.POINTER(Rect),w.DWORD]
        api.MonitorFromRect.restype=w.HANDLE
        api.GetMonitorInfoW.argtypes=[w.HANDLE,c.POINTER(Info)]
        info=Info(size=c.sizeof(Info)); rect=Rect(*query)
        monitor=api.MonitorFromRect(c.byref(rect),0)
        if not monitor or not api.GetMonitorInfoW(monitor,c.byref(info)):
            return None
        shift=[plan["roi"][0]-query[0],plan["roi"][1]-query[1]]
        area=[info.work.left+shift[0],info.work.top+shift[1],info.work.right+shift[0],info.work.bottom+shift[1]]
        sidecar.write_text(json.dumps({"work_area":area,"origin":"verification-time monitor query; unchanged test layout / saved layout device mapping"},indent=2))
        return area
    finally:
        api.SetThreadDpiAwarenessContext(old)


def verify_inner(folder, ffmpeg, ffprobe):
    plan = json.loads((folder / "plan.json").read_text(encoding="utf-8-sig"))
    width = plan["roi"][2] - plan["roi"][0]
    height = plan["roi"][3] - plan["roi"][1]
    area=work_area(folder,plan)
    results = []
    for path in sorted(folder.glob("round-*.json")):
        report = json.loads(path.read_text(encoding="utf-8-sig"))
        if "error" in report:
            if report.get("expected_failure"):
                assert not path.with_suffix(".mp4").exists() and not path.with_suffix(".partial.mp4").exists()
                if plan.get("spec",{}).get("fault") in ["write","finalize"]:
                    quota=re.search(r"quota_failed_writes=(\d+)",report["error"])
                    assert quota and int(quota.group(1))>0, "Native stream did not observe a failed write"
            results.append({"file": path.name, "passed": report.get("expected_failure", False), "error": report["error"]})
            continue
        if report["cancelled"]:
            assert not path.with_suffix(".mp4").exists()
            assert not path.with_suffix(".partial.mp4").exists()
            results.append({"file": path.name, "cancelled": True, "passed": True})
            continue
        video = path.with_suffix(".mp4")
        probe = json.loads(invoke([ffprobe, "-v", "error", "-show_streams", "-show_format", "-of", "json", str(video)]))
        streams = probe["streams"]
        assert len(streams) == 1 and streams[0]["codec_type"] == "video"
        stream = streams[0]
        assert stream["codec_name"] == "h264"
        assert stream["profile"] == "High"
        assert stream.get("color_range") == "tv"
        assert (stream["width"], stream["height"]) == (width, height)
        fps = report["fps"]
        duration = float(stream["duration"])
        assert abs(duration - report["active_ms"] / 1000) <= 1 / fps + float(Fraction(stream["time_base"]))
        # Full independent decode checks the tail and corrupt/truncated samples.
        invoke([ffmpeg, "-v", "error", "-xerror", "-i", str(video), "-f", "null", "-"])
        markers = invoke([ffmpeg, "-v", "error", "-i", str(video), "-vf",
                          "crop=512:32:64:64,scale=16:1:flags=area,format=gray",
                          "-fps_mode", "passthrough", "-f", "rawvideo", "-"])
        samples = report.get("samples")
        if samples is None:
            samples = [json.loads(line) for line in (folder / report["samples_file"]).open(encoding="utf-8-sig") if line.strip()]
        assert report["frames"] == len(samples), "Reported frame count differs from submitted samples"
        actual = json.loads(invoke([ffprobe, "-v", "error", "-select_streams", "v:0", "-show_frames", "-show_entries", "frame=best_effort_timestamp_time,pkt_duration_time,duration_time,color_range,color_space,color_transfer,color_primaries", "-of", "json", str(video)]))["frames"]
        assert len(actual) == len(samples)
        for frame, expected in zip(actual, samples):
            assert abs(float(frame["best_effort_timestamp_time"]) - expected["pts_100ns"] / 10000000) < .0002
            actual_duration=frame.get("duration_time",frame.get("pkt_duration_time"))
            assert actual_duration is not None and abs(float(actual_duration)-expected["duration_100ns"]/10000000)<.0002
            assert frame.get("color_space")=="bt709", "Frame-level BT.709 metadata missing or incorrect"
            assert frame.get("color_range")=="tv" and frame.get("color_primaries")=="bt709" and frame.get("color_transfer")=="bt709", "Frame-level color contract not met"
        assert len(markers) == len(samples) * 16
        previous_end = 0
        previous_sample = None
        for index, sample in enumerate(samples):
            bits = markers[index * 16:(index + 1) * 16]
            decoded = sum((value > 128) << bit for bit, value in enumerate(bits))
            assert decoded == sample["marker"], (path.name, index, decoded, sample["marker"])
            assert sample["pts_100ns"] == previous_end
            previous_end += sample["duration_100ns"]
            if previous_sample is not None:
                assert sample["source_epoch"] >= previous_sample["source_epoch"]
                assert sample["source_100ns"] >= previous_sample["source_100ns"]
                if sample["source_epoch"] > previous_sample["source_epoch"]:
                    pause_ticks=plan.get("spec",{}).get("pause_ms",2000)*10000
                    assert sample["source_100ns"]-previous_sample["source_100ns"] >= pause_ticks-100000, "Resume reused a pre-pause source"
            previous_sample=sample
        dynamic_ms = plan.get("spec", {}).get("dynamic_ms", 6000)
        dynamic = [s for s in samples if 200 <= s["at_ms"] < dynamic_ms]
        changes = sum(a["marker"] != b["marker"] for a, b in zip(dynamic, dynamic[1:]))
        content_fps = changes / ((dynamic[-1]["at_ms"] - dynamic[0]["at_ms"]) / 1000) if len(dynamic) > 1 else 0
        if dynamic_ms == 0:
            static = [s for s in samples if 300 <= s["at_ms"] < plan["spec"]["static_ms"]]
            assert static and len({s["marker"] for s in static}) == 1, "Static scene did not remain static"
        # Original full-size RGB frame, rather than the tiny marker stream, tests exclusion.
        rgb = invoke([ffmpeg, "-v", "error", "-i", str(video), "-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        def pixel(x, y):
            offset = (y * width + x) * 3
            return list(rgb[offset:offset + 3])
        # Borders, in-region controls and hint all cover this known dark background.
        patches = [(1, 10), (width - 2, 10), (200, 220), (650, 220)]
        for x, y in patches:
            assert all(abs(a - b) <= 8 for a, b in zip(pixel(x, y), [32, 52, 76])), (path.name, x, y, pixel(x, y))
        points = [(1, 10), (width - 2, 10), (20, 1), (20, height - 2), (200, 220), (650, 220), (180, 400), (430, 400), (680, 400)]
        expected_colors = [[32, 52, 76]] * 6 + [[220, 50, 70], [45, 180, 90], [50, 100, 220]]
        labels = "".join(f"[s{i}]" for i in range(len(points)))
        graph = f"[0:v]split={len(points)}{labels};" + ";".join(
            f"[s{i}]crop=2:2:{x // 2 * 2}:{y // 2 * 2},scale=1:1:flags=area[p{i}]" for i, (x, y) in enumerate(points))
        graph += ";" + "".join(f"[p{i}]" for i in range(len(points))) + f"hstack=inputs={len(points)},format=rgb24[out]"
        patches_raw = invoke([ffmpeg, "-v", "error", "-i", str(video), "-filter_complex", graph, "-map", "[out]", "-fps_mode", "passthrough", "-f", "rawvideo", "-"])
        assert len(patches_raw) == len(samples) * len(points) * 3
        shell_colors=set()
        for offset in range(0, len(patches_raw), len(points) * 3):
            for patch, color in enumerate(expected_colors):
                measured=patches_raw[offset+patch*3:offset+patch*3+3]
                x,y=points[patch]
                screen_x,screen_y=plan["roi"][0]+x,plan["roi"][1]+y
                if patch==3 and area and not (area[0]<=screen_x<area[2] and area[1]<=screen_y<area[3]):
                    # The taskbar is real desktop content. Only its color differs
                    # from the synthetic source; cyan range feedback must stay out.
                    assert not (measured[0]<80 and measured[1]>180 and measured[2]>180), ("Range border leaked over shell",offset)
                    if len(shell_colors)<256:
                        shell_colors.add(tuple(measured))
                    continue
                assert all(abs(a-b) <= 10 for a,b in zip(measured,color)), (path.name, offset // (len(points)*3), patch, list(measured))
        if report.get("cursor_path"):
            cursor = invoke([ffmpeg, "-v", "error", "-i", str(video), "-vf", "crop=160:64:1090:790,format=gray", "-fps_mode", "passthrough", "-f", "rawvideo", "-"])
            assert len(cursor) == len(samples) * 160 * 64
            spec=plan.get("spec",{})
            pause=spec.get("dynamic_ms",6000)+spec.get("static_ms",6000)
            resume=pause+spec.get("pause_ms",2000)
            stop=resume+spec.get("resume_ms",2000)
            checked=0
            for index,sample in enumerate(samples):
                # Real control clicks intentionally move the pointer out of the
                # source path. Inspect stable intervals rather than mislabel that
                # expected UI motion as a missing captured system cursor.
                if spec.get("native_controls") and any(abs(sample["at_ms"]-boundary)<100 for boundary in [pause,resume,stop]):
                    continue
                if sample["at_ms"] < 300:
                    continue
                checked+=1
                pixels = cursor[index*160*64:(index+1)*160*64]
                assert max(pixels) > 200 and min(pixels) < 20, ("System cursor absent", index)
            assert checked >= len(samples)*.7, "Too few stable cursor frames checked"
        proof = path.with_suffix(".decoded.png")
        invoke([ffmpeg, "-v", "error", "-y", "-i", str(video), "-frames:v", "1", str(proof)])
        digest = hashlib.sha256()
        with video.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
        results.append({"file": path.name, "passed": content_fps >= 27 if fps == 30 and dynamic_ms > 0 else True,
                        "shell_edge_colors":sorted(shell_colors),
                        "content_fps": content_fps, "duration": duration, "frames": len(samples),
                        "first_frame_ms": report["first_frame_ms"],
                        "stop_to_closed_ms": report["stop_to_closed_ms"],
                        "stop_to_ready_ms": report.get("stop_to_ready_ms", report["stop_to_closed_ms"]),
                        "sha256": digest.hexdigest(),
                        "encoder": report["encoder"],
                        "stages_ms": {key: {"median": statistics.median([s[key] for s in samples]),
                                              "p95": percentile([s[key] for s in samples], .95)}
                                      for key in ["read_ms", "convert_ms", "write_ms", "source_age_ms"]}})
    assert results, "No completed native runs"
    assert len(results)==plan["rounds"], "Missing native run reports"
    for retired in folder.glob("retired-*.json"):
        assert all(value==0 for value in json.loads(retired.read_text(encoding="utf-8-sig")).values()), "Owned resources did not retire"
    summary = {"passed": all(r["passed"] for r in results), "results": results}
    successful = [r for r in results if "first_frame_ms" in r]
    if successful:
        summary["timings_ms"] = {key: {"median": statistics.median([r[key] for r in successful]),
                                         "p95": percentile([r[key] for r in successful], .95)}
                                 for key in ["first_frame_ms", "stop_to_closed_ms", "stop_to_ready_ms"]}
        summary["stop_gate_passed"] = summary["timings_ms"]["stop_to_ready_ms"]["p95"] <= 2000
        summary["passed"] &= summary["stop_gate_passed"]
    (folder / "decode-report.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    return summary["passed"]


def verify(folder, ffmpeg, ffprobe):
    start=time.monotonic()
    try:
        passed = verify_inner(folder, ffmpeg, ffprobe)
        path = folder / "decode-report.json"
        report = json.loads(path.read_text(encoding="utf-8"))
        report["check_seconds"] = time.monotonic() - start
        path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report, indent=2))
        return passed
    except Exception as error:
        result={"passed":False,"error":str(error),"traceback":traceback.format_exc(),"check_seconds":time.monotonic()-start}
        (folder / "decode-report.json").write_text(json.dumps(result,indent=2),encoding="utf-8")
        print(json.dumps(result,indent=2))
        return False


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=pathlib.Path)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--ffprobe", default="ffprobe")
    args = parser.parse_args()
    raise SystemExit(0 if verify(args.folder, args.ffmpeg, args.ffprobe) else 1)
