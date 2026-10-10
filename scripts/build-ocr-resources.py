"""Build the local Windows OCR resource archive; never executed by the application.

Run in an isolated build environment with PaddlePaddle, Paddle2ONNX, PyYAML and
ONNX Runtime installed. Inputs are pinned official PaddlePaddle model revisions.
The output archive and generated integrity manifest must be reviewed before publishing.
"""
from pathlib import Path
import argparse
import os
import hashlib
import json
import subprocess
import sys
import urllib.request
import zipfile
import yaml
import onnxruntime

MODELS = {
    "det": ("PP-OCRv5_mobile_det", "0d63e78e2b680928f6b1747d76a08db6e645efb7"),
    "common": ("PP-OCRv5_mobile_rec", "682f20538d8c086cb2128e5cfac775e6c4904e85"),
    "latin": ("latin_PP-OCRv5_mobile_rec", "ab2cd5cc5fa6309be2e5acdfe66eca2c2c127d57"),
    "korean": ("korean_PP-OCRv5_mobile_rec", "c02ecaf1f22bfd1c618cce154fd19185b47e663a"),
}

def fetch(url, destination):
    if not destination.exists():
        with urllib.request.urlopen(url, timeout=120) as response:
            destination.write_bytes(response.read())

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path("target/ocr-resources"))
    parser.add_argument("--manifest", type=Path, default=Path("crates/lexift-platform/src/ocr/assets.json"))
    parser.add_argument("--runtime-dir", type=Path)
    args = parser.parse_args()
    build = args.output.resolve()
    files = build / "files"
    files.mkdir(parents=True, exist_ok=True)
    for key, (name, revision) in MODELS.items():
        source = build / name
        source.mkdir(exist_ok=True)
        for filename in ("inference.json", "inference.pdiparams", "inference.yml"):
            fetch(f"https://huggingface.co/PaddlePaddle/{name}/resolve/{revision}/{filename}", source / filename)
        model = files / f"{key}.onnx"
        if not model.exists():
            subprocess.run([str(Path(sys.executable).parent / ("paddle2onnx.exe" if os.name == "nt" else "paddle2onnx")), "--model_dir", str(source), "--model_filename", "inference.json", "--params_filename", "inference.pdiparams", "--save_file", str(model), "--opset_version", "17"], check=True)
        # Reject incompatible conversion before putting it in a release archive.
        session = onnxruntime.InferenceSession(str(model), providers=["CPUExecutionProvider"])
        print(key, [(i.name, i.shape) for i in session.get_inputs()], flush=True)
        if key != "det":
            config = yaml.safe_load((source / "inference.yml").read_text(encoding="utf-8"))
            dictionary = config["PostProcess"]["character_dict"]
            (files / f"{key}.txt").write_text("\n".join(dictionary) + "\n", encoding="utf-8")
    runtime = args.runtime_dir or Path(onnxruntime.__file__).parent / "capi"
    for name in ("onnxruntime.dll", "onnxruntime_providers_shared.dll"):
        (files / name).write_bytes((runtime / name).read_bytes())
    fetch("https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/v3.0.2/LICENSE", files / "LICENSE-PaddleOCR.txt")
    fetch("https://raw.githubusercontent.com/microsoft/onnxruntime/v" + onnxruntime.__version__ + "/LICENSE", files / "LICENSE-ONNXRuntime.txt")
    fetch("https://raw.githubusercontent.com/microsoft/onnxruntime/v" + onnxruntime.__version__ + "/ThirdPartyNotices.txt", files / "NOTICES-ONNXRuntime.txt")
    archive = build / "lexift-ocr-v1-windows-x86_64.zip"
    entries = []
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
        for file in sorted(files.iterdir()):
            data = file.read_bytes()
            entries.append({"name": file.name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
            info = zipfile.ZipInfo(file.name, (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            bundle.writestr(info, data)
    data = archive.read_bytes()
    manifest = {"version": "v1", "url": "https://github.com/lizhenisu/lexift/releases/download/ocr-v1/" + archive.name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "files": entries, "models": MODELS, "runtime_version": onnxruntime.__version__}
    args.manifest.parent.mkdir(parents=True, exist_ok=True)
    args.manifest.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(archive, len(data), "bytes", manifest["sha256"], flush=True)

if __name__ == "__main__":
    main()
