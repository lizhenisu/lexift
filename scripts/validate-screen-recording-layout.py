"""Owned native layout tests; always restore the secondary display's saved mode."""
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time


class Mode(c.Structure):
    _fields_ = [("device", w.WCHAR * 32), ("spec", w.WORD), ("driver", w.WORD),
                ("size", w.WORD), ("extra", w.WORD), ("fields", w.DWORD),
                ("x", w.LONG), ("y", w.LONG), ("orientation", w.DWORD), ("fixed", w.DWORD),
                ("color", c.c_short), ("duplex", c.c_short), ("resolution", c.c_short),
                ("tt", c.c_short), ("collate", c.c_short), ("form", w.WCHAR * 32), ("dpi", w.WORD)] + [
                    (name, w.DWORD) for name in ["bits", "width", "height", "flags", "hz", "icm", "intent", "media", "dither", "r1", "r2", "panx", "pany"]]


class Device(c.Structure):
    _fields_ = [("cb", w.DWORD), ("name", w.WCHAR * 32), ("description", w.WCHAR * 128),
                ("flags", w.DWORD), ("identifier", w.WCHAR * 128), ("key", w.WCHAR * 128)]


def main(exe, root):
    api = c.WinDLL("user32", use_last_error=True)
    api.EnumDisplayDevicesW.argtypes = [w.LPCWSTR, w.DWORD, c.POINTER(Device), w.DWORD]
    api.EnumDisplaySettingsExW.argtypes = [w.LPCWSTR, w.DWORD, c.POINTER(Mode), w.DWORD]
    api.ChangeDisplaySettingsExW.argtypes = [w.LPCWSTR, c.POINTER(Mode), w.HWND, w.DWORD, c.c_void_p]
    api.ChangeDisplaySettingsExW.restype = w.LONG
    assert c.sizeof(Mode) == 220
    devices = []
    for index in range(32):
        device = Device(cb=c.sizeof(Device))
        if not api.EnumDisplayDevicesW(None, index, c.byref(device), 0):
            break
        if device.flags & 1:
            devices.append(device)
    secondary = next(d for d in devices if not d.flags & 4)
    saved = Mode(size=c.sizeof(Mode))
    assert api.EnumDisplaySettingsExW(secondary.name, 0xffffffff, c.byref(saved), 0)
    changed = Mode.from_buffer_copy(bytes(saved))
    changed.x, changed.y = -int(changed.width), 0
    changed.fields = 0x20  # DM_POSITION; no registry update or persistent mode change.
    root.mkdir(parents=True, exist_ok=False)
    (root / "saved-mode.bin").write_bytes(bytes(saved))
    (root / "layout.json").write_text(json.dumps({"device": secondary.name, "original": [saved.x, saved.y, saved.width, saved.height], "negative": [changed.x, changed.y]}))

    def apply(mode):
        status = api.ChangeDisplaySettingsExW(secondary.name, c.byref(mode), None, 0, None)
        assert status == 0, ("display change rejected", status)

    def start(name, expected=None):
        folder = root / name
        folder.mkdir()
        plan = {"roi": [changed.x, 0, 0, int(changed.height)], "rounds": 1,
                "spec": {"dynamic_ms": 5000 if expected else 2000, "static_ms": 1000, "pause_ms": 500, "resume_ms": 1000}}
        if expected:
            plan["expected_error"] = expected
        (folder / "plan.json").write_text(json.dumps(plan))
        (folder / "build.json").write_text(json.dumps({"sha256": hashlib.sha256(exe.read_bytes()).hexdigest()}))
        env = dict(os.environ, LEXIFT_RECORDING_GATE_DIR=str(folder.resolve()))
        log = (folder / "native-test.log").open("wb")
        process = subprocess.Popen([str(exe.resolve()), "windows::recording_gate::native_recording_matrix", "--exact", "--ignored", "--nocapture"], env=env, stdout=log, stderr=subprocess.STDOUT)
        return process, log, folder

    try:
        assert api.ChangeDisplaySettingsExW(secondary.name, c.byref(changed), None, 2, None) == 0
        apply(changed)
        process, log, folder = start("negative")
        try:
            assert process.wait(timeout=20) == 0
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait()
            log.close()
        process, log, folder = start("layout-change", "Recording monitor layout changed")
        try:
            deadline = time.monotonic() + 10
            sample = folder / "round-00.partial.samples.jsonl"
            while time.monotonic() < deadline and process.poll() is None:
                if sample.exists() and sample.stat().st_size > 0:
                    break
                time.sleep(.02)
            assert process.poll() is None and sample.exists() and sample.stat().st_size > 0
            apply(saved)
            assert process.wait(timeout=20) == 0
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait()
            log.close()
    finally:
        apply(saved)
        restored = Mode(size=c.sizeof(Mode))
        assert api.EnumDisplaySettingsExW(secondary.name, 0xffffffff, c.byref(restored), 0)
        assert (restored.x, restored.y, restored.width, restored.height) == (saved.x, saved.y, saved.width, saved.height)
        (root / "restored.json").write_text(json.dumps({"restored": True}))


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]))
