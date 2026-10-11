"""Build the local Windows OCR resource archive; never executed by the application.

Run in an isolated build environment with PaddlePaddle, Paddle2ONNX, PyYAML and
ONNX Runtime installed. Tested exporter: PaddlePaddle 3.1.1 + Paddle2ONNX
2.0.2rc3, Python 3.12, ONNX Runtime 1.22.0. Inputs are pinned official PaddlePaddle model revisions.
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
    "universal": ("PP-OCRv6_small_rec", "bd619643acac4b9650c040234da8d944476ee3f1"),
    "korean": ("korean_PP-OCRv5_mobile_rec", "c02ecaf1f22bfd1c618cce154fd19185b47e663a"),
}

def fetch(url, destination):
    if not destination.exists():
        with urllib.request.urlopen(url, timeout=120) as response:
            destination.write_bytes(response.read())

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path("target/ocr-resources-v1"))
    parser.add_argument("--manifest", type=Path, default=Path("crates/lexift-platform/src/ocr/assets.json"))
    parser.add_argument("--runtime-dir", type=Path)
    parser.add_argument("--source-files", type=Path, help="Reuse previously exported files after release-contract verification")
    args = parser.parse_args()
    build = args.output.resolve()
    files = build / "files"
    files.mkdir(parents=True, exist_ok=True)
    if args.source_files:
        write_packages(build, args.source_files.resolve(), args.manifest)
        return
    for key, (name, revision) in MODELS.items():
        source = build / name
        source.mkdir(exist_ok=True)
        for filename in ("inference.json", "inference.pdiparams", "inference.yml"):
            fetch(f"https://huggingface.co/PaddlePaddle/{name}/resolve/{revision}/{filename}", source / filename)
        model = files / f"{key}.onnx"
        exporting = not model.exists()
        if exporting:
            model = files / f"{key}.export.onnx"
            subprocess.run([str(Path(sys.executable).parent / ("paddle2onnx.exe" if os.name == "nt" else "paddle2onnx")), "--model_dir", str(source), "--model_filename", "inference.json", "--params_filename", "inference.pdiparams", "--save_file", str(model), "--opset_version", "17"], check=True)
        # Export contract: Paddle's inference model removes NRTR and emits CTC probabilities.
        # Verify the actual tensor shape and dictionary against a CPU inference, not names.
        import numpy as np
        # Reject incompatible conversion before putting it in a release archive.
        session = onnxruntime.InferenceSession(str(model), providers=["CPUExecutionProvider"])
        print(key, [(i.name, i.shape) for i in session.get_inputs()], flush=True)
        if key != "det":
            config = yaml.safe_load((source / "inference.yml").read_text(encoding="utf-8"))
            assert config["PostProcess"]["name"] == "CTCLabelDecode"
            assert config["PreProcess"]["transform_ops"][0]["DecodeImage"]["img_mode"] == "BGR"
            dictionary = config["PostProcess"]["character_dict"]
            prediction = session.run(None, {session.get_inputs()[0].name: np.zeros((1, 3, 48, 320), np.float32)})
            expected_classes = 1 + len(dictionary) + (dictionary[-1] != " ")
            assert len(prediction) == 1 and prediction[0].ndim == 3
            assert prediction[0].shape[0] == 1 and prediction[0].shape[2] == expected_classes
            assert np.isfinite(prediction[0]).all()
            assert np.allclose(prediction[0].sum(axis=2), 1, atol=1e-3)
            (files / f"{key}.txt").write_text("\n".join(dictionary) + "\n", encoding="utf-8")
        if exporting:
            model.replace(files / f"{key}.onnx")
    runtime = args.runtime_dir or Path(onnxruntime.__file__).parent / "capi"
    for name in ("onnxruntime.dll", "onnxruntime_providers_shared.dll"):
        (files / name).write_bytes((runtime / name).read_bytes())
    fetch("https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/v3.0.2/LICENSE", files / "LICENSE-PaddleOCR.txt")
    fetch("https://raw.githubusercontent.com/microsoft/onnxruntime/v" + onnxruntime.__version__ + "/LICENSE", files / "LICENSE-ONNXRuntime.txt")
    fetch("https://raw.githubusercontent.com/microsoft/onnxruntime/v" + onnxruntime.__version__ + "/ThirdPartyNotices.txt", files / "NOTICES-ONNXRuntime.txt")
    write_packages(build, files, args.manifest)

def write_packages(build, files, base_manifest):
    """Create independently verifiable packages sharing one pinned CPU runtime."""
    import numpy as np
    # Batch support is a release contract, tested before declaring it in a manifest.
    for key in ("universal", "korean"):
        session = onnxruntime.InferenceSession(str(files / f"{key}.onnx"), providers=["CPUExecutionProvider"])
        dictionary = (files / f"{key}.txt").read_text(encoding="utf-8").splitlines()
        expected_classes = 1 + len(dictionary) + (dictionary[-1] != " ")
        for batch in (1, 2, 4):
            for width in (96, 320):
                outputs = session.run(None, {session.get_inputs()[0].name: np.zeros((batch, 3, 48, width), np.float32)})
                assert len(outputs) == 1
                prediction = outputs[0]
                assert prediction.ndim == 3 and prediction.shape[0] == batch
                assert prediction.shape[2] == expected_classes and np.isfinite(prediction).all()
                assert np.allclose(prediction.sum(axis=2), 1, atol=1e-3)
    for kind, version, names in (
        ("base", "v1", {"det.onnx", "universal.onnx", "universal.txt", "onnxruntime.dll", "onnxruntime_providers_shared.dll", "LICENSE-PaddleOCR.txt", "LICENSE-ONNXRuntime.txt", "NOTICES-ONNXRuntime.txt"}),
        ("korean", "korean-v1", {"korean.onnx", "korean.txt", "LICENSE-PaddleOCR.txt"}),
    ):
        archive = build / f"lexift-ocr-{version}-windows-x86_64.zip"
        entries = []
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
            for name in sorted(names):
                data = (files / name).read_bytes()
                entries.append({"name": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
                info = zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                bundle.writestr(info, data)
        data = archive.read_bytes()
        manifest = {"version": version, "url": "https://github.com/lizhenisu/lexift/releases/download/ocr-v1/" + archive.name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(), "files": entries, "models": {k:v for k,v in MODELS.items() if (k == "korean") == (kind == "korean")}, "runtime_version": onnxruntime.__version__, "compatible_base": "v1", "recognition_batch": 4}
        destination = base_manifest if kind == "base" else base_manifest.with_name("korean-assets.json")
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        print(archive, len(data), "bytes", manifest["sha256"], flush=True)

if __name__ == "__main__":
    main()
