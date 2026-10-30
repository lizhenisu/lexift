"""Prove the independent video checker rejects corrupted or missing gate evidence."""
import argparse
import contextlib
import importlib.util
import io
import json
import pathlib
import shutil
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("evidence", type=pathlib.Path)
    parser.add_argument("--ffmpeg", required=True)
    parser.add_argument("--ffprobe", required=True)
    args = parser.parse_args()
    args.evidence.mkdir(parents=True, exist_ok=False)
    verifier = pathlib.Path(__file__).with_name("validate-screen-recording.py")
    results = []
    for case in ["truncated", "wrong-frame-count", "missing-video", "injected-decoded-pixels"]:
        folder = args.evidence / case
        folder.mkdir()
        plan = json.loads((args.source / "plan.json").read_text(encoding="utf-8-sig"))
        plan["rounds"] = 1
        (folder / "plan.json").write_text(json.dumps(plan), encoding="utf-8")
        report = json.loads((args.source / "round-00.json").read_text(encoding="utf-8-sig"))
        if case == "wrong-frame-count":
            report["frames"] += 1
        (folder / "round-00.json").write_text(json.dumps(report), encoding="utf-8")
        for name in [report.get("samples_file"), "retired-00.json", "verification-layout.json"]:
            if name and (args.source / name).exists():
                shutil.copyfile(args.source / name, folder / name)
        if case != "missing-video":
            shutil.copyfile(args.source / "round-00.mp4", folder / "round-00.mp4")
        if case == "truncated":
            video = folder / "round-00.mp4"
            with video.open("r+b") as stream:
                stream.truncate(video.stat().st_size // 2)
        if case == "injected-decoded-pixels":
            # This is deliberately a checker injection, not an encoder failure.
            spec = importlib.util.spec_from_file_location("recording_verifier", verifier)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            original = module.invoke

            def inject(command):
                data = original(command)
                if "-frames:v" in command and "rgb24" in command:
                    return bytes(len(data))
                return data

            module.invoke = inject
            with contextlib.redirect_stdout(io.StringIO()):
                detected = not module.verify(folder, args.ffmpeg, args.ffprobe)
        else:
            with (folder / "checker.log").open("wb") as output:
                result = subprocess.run(
                    [sys.executable, str(verifier), str(folder), "--ffmpeg", args.ffmpeg,
                     "--ffprobe", args.ffprobe], stdout=output, stderr=subprocess.STDOUT,
                    check=False,
                )
            detected = result.returncode != 0
        failure = json.loads((folder / "decode-report.json").read_text(encoding="utf-8"))
        results.append({"case": case, "detected": detected and failure["passed"] is False,
                        "injected": case == "injected-decoded-pixels"})
    summary = {"passed": all(result["detected"] for result in results), "results": results}
    (args.evidence / "matrix.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
