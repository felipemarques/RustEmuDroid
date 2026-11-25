"""Exercise Home scrolling, profiles, Android frame counting and capture on Linux."""
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET
import zipfile


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, timeout=35, **kwargs).strip()


def wait_for(description, check, timeout=60):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(0.5)
    raise RuntimeError(f"Timed out waiting for {description}")


def window_size(window):
    geometry = dict(line.split("=", 1) for line in
        run("xdotool", "getwindowgeometry", "--shell", window).splitlines() if "=" in line)
    return int(geometry["WIDTH"]), int(geometry["HEIGHT"])


def latest_frame_size(log_path, offset=0):
    with log_path.open("rb") as log:
        log.seek(offset)
        samples = re.findall(rb"(?m)^display_frames=[1-9][0-9]* interval_ms=[1-9][0-9]* size=([0-9]+)x([0-9]+)\r?\n",
                             log.read())
    return tuple(map(int, samples[-1])) if samples else None


def png_size(header):
    if len(header) < 24 or header[:16] != b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR":
        raise ValueError("Invalid PNG signature or IHDR")
    width, height = struct.unpack(">II", header[16:24])
    if not width or not height:
        raise ValueError("PNG has an empty IHDR extent")
    return width, height


def check_adaptive_capture(window, log_path, captures, native):
    before_resize = wait_for("initial desktop frame statistics", lambda: latest_frame_size(log_path), 20)
    offset = log_path.stat().st_size
    # The launcher has a 610 x 510 minimum in logical points. This physical window
    # still has clearly fewer fitted pixels than either orientation of the guest.
    run("xdotool", "windowsize", "--sync", window, "620", "520")
    client = window_size(window)

    def reduced_size():
        size = latest_frame_size(log_path, offset)
        if size and size != before_resize and all(0 < edge < original and edge <= bound + 16
                for edge, original, bound in zip(size, native, client)):
            return size

    reduced = wait_for("a new reduced video raster after resizing", reduced_size, 25)
    existing = set(captures.glob("*.png"))
    run("xdotool", "windowfocus", "--sync", window)
    run("xdotool", "key", "--clearmodifiers", "ctrl+shift+s")

    def exported_png():
        new_files = set(captures.glob("*.png")) - existing
        assert len(new_files) <= 1, "One screenshot shortcut exported multiple PNGs"
        return next(iter(new_files), None)

    screenshot = wait_for("the PNG exported by Ctrl+Shift+S", exported_png, 25)
    with screenshot.open("rb") as image:
        actual = png_size(image.read(24))
    assert actual == native, f"Expected native PNG {native}, got {actual} from reduced raster {reduced}"
    offset = log_path.stat().st_size
    wait_for("reduced video to return after PNG export", lambda: latest_frame_size(log_path, offset) == reduced, 25)
    print(f"Adaptive video: client {client[0]}x{client[1]}, reduced {reduced[0]}x{reduced[1]}, "
          f"Ctrl+Shift+S PNG IHDR {actual[0]}x{actual[1]}, reduced video resumed", flush=True)


def build_fixtures(work, sdk):
    tools = max((sdk / "build-tools").glob("[0-9]*"), key=lambda p: tuple(map(int, re.findall(r"\d+", p.name))))
    jar = max(sdk.glob("platforms/android-*/android.jar"), key=lambda p: int(re.search(r"android-(\d+)", str(p)).group(1)))
    key = work / "test.jks"
    run("keytool", "-genkeypair", "-keystore", key, "-storepass", "fixture", "-keypass", "fixture",
        "-alias", "fixture", "-keyalg", "RSA", "-dname", "CN=CI fixture", "-validity", "2")
    for kind in ("Games", "Apps"):
        package = f"org.rustemudroid.qa.{kind.lower()}"
        game = kind == "Games"
        activity = f"{package}.FrameActivity" if game else "android.app.Activity"
        manifest = ET.Element("manifest", {"xmlns:android": "http://schemas.android.com/apk/res/android", "package": package})
        ET.SubElement(manifest, "uses-sdk", {"android:minSdkVersion": "26", "android:targetSdkVersion": "32"})
        app = ET.SubElement(manifest, "application", {"android:hasCode": str(game).lower(), "android:appCategory": "game" if game else "productivity"})
        ET.SubElement(app, "activity", {"android:name": activity, "android:exported": "false"})
        for i in range(40):
            alias = ET.SubElement(app, "activity-alias", {"android:name": f".Card{i}", "android:label": f"AA {kind} {i:02d}",
                "android:targetActivity": activity, "android:exported": "true"})
            intent = ET.SubElement(alias, "intent-filter")
            ET.SubElement(intent, "action", {"android:name": "android.intent.action.MAIN"})
            ET.SubElement(intent, "category", {"android:name": "android.intent.category.LAUNCHER"})
        path = work / "AndroidManifest.xml"
        ET.ElementTree(manifest).write(path, encoding="utf-8")
        unsigned = work / f"{kind}-unsigned.apk"
        aligned = work / f"{kind}-aligned.apk"
        signed = work / f"{kind}.apk"
        run(tools / "aapt", "package", "-f", "-M", path, "-I", jar, "-F", unsigned)
        if game:
            run("javac", "--release", "8", "-cp", jar, "-d", work, Path(__file__).with_name("FrameActivity.java"))
            run(tools / "d8", "--lib", jar, "--min-api", "26", "--output", work,
                work / "org/rustemudroid/qa/games/FrameActivity.class")
            with zipfile.ZipFile(unsigned, "a", compression=zipfile.ZIP_DEFLATED) as apk:
                apk.write(work / "classes.dex", "classes.dex")
        run(tools / "zipalign", "-f", "4", unsigned, aligned)
        run(tools / "apksigner", "sign", "--ks", key, "--ks-pass", "pass:fixture", "--out", signed, aligned)
        yield signed


def main():
    temp = Path(os.environ["RUNNER_TEMP"])
    report = temp / "boot-report"
    app = temp / "rustemudroid"
    state = temp / "profile"
    adb = app / "runtime/platform-tools/adb"
    avd = state / "avd/RustEmuDroid.avd"
    assert avd.is_dir()
    with tempfile.TemporaryDirectory() as tmp:
        fixtures = list(build_fixtures(Path(tmp), Path(os.environ["ANDROID_HOME"])))
        env = {k: v for k, v in os.environ.items() if k not in ("ANDROID_HOME", "ANDROID_SDK_ROOT", "ANDROID_AVD_HOME", "ANDROID_USER_HOME", "WAYLAND_DISPLAY")}
        env["HOME"] = str(temp / "guest-home")
        env["RUSTEMUDROID_FRAME_STATS"] = "1"
        def adb_run(*args):
            return run(adb, *args, env=env)
        assert adb_run("devices").splitlines() == ["List of devices attached"], "Requires an idle runner"
        # Exercise the real packaged window at phone density within the software renderer's budget.
        config = avd / "config.ini"
        values = {"hw.lcd.width": 540, "hw.lcd.height": 960, "hw.lcd.density": 210}
        settings = [line for line in config.read_text().splitlines() if line.partition("=")[0].strip() not in values]
        settings.extend(f"{key}={value}" for key, value in values.items())
        config.write_text("\n".join(settings) + "\n")
        prefs = state / "launcher.json"
        launcher = {
            "sdk": "", "state": str(state), "image": "", "gpu": "Auto",
            "hardware": {"adapter": "", "camera": "Profile", "webcam_index": 0},
            "show_fps": True,
        }
        if prefs.is_file():
            launcher.update(json.loads(prefs.read_text()))
        launcher["adaptive_video"] = True
        prefs.write_text(json.dumps(launcher) + "\n")
        game = "org.rustemudroid.qa.games"
        maps = avd / "touch-maps.json"
        with maps.open("x") as output:
            empty = {"buttons": {}, "stick": None}
            json.dump({game: {"active": "On foot", "layouts": {"Driving": empty, "On foot": empty}}}, output)
        desktop_log = report / "home-wheel-desktop.txt"
        with desktop_log.open("wb") as log:
            client = subprocess.Popen([app / "rustemudroid", "--lang", "en-us", "--state-dir", state,
                "ui", "--gpu", "swiftshader"], env=env, stdout=log, stderr=subprocess.STDOUT)
        serial = None
        last_tree = None
        seen = {}
        try:
            def owned_device():
                if client.poll() is not None:
                    raise RuntimeError(f"Desktop exited with {client.returncode}")
                for candidate in re.findall(r"^(emulator-\d+)\s+device\s*$", adb_run("devices"), re.M):
                    reply = adb_run("-s", candidate, "emu", "avd", "path").splitlines()
                    if seen.get(candidate) != reply:
                        print(f"{candidate}: {reply}", flush=True)
                        seen[candidate] = reply
                    if reply and Path(reply[0]) == avd:
                        return candidate
            serial = wait_for("owned AVD", owned_device, 150)
            def guest(*args):
                return adb_run("-s", serial, *args)
            home = "org.rustemudroid.home"
            wait_for("installed Home", lambda: "versionCode=4 " in guest("shell", "dumpsys", "package", home), 150)
            guest("shell", "input", "keyevent", "KEYCODE_WAKEUP")
            guest("shell", "wm", "dismiss-keyguard")
            for apk in fixtures:
                assert "Success" in guest("install", "-r", apk)
            window = run("xdotool", "search", "--onlyvisible", "--name", "^RustEmuDroid").splitlines()[-1]
            run("xdotool", "windowfocus", "--sync", window)
            width, height = window_size(window)
            run("xdotool", "mousemove", "--window", window, str(width // 2), str(height * 3 // 5))
            system_waits = 0
            def cards(kind):
                nonlocal system_waits, last_tree
                path = "/data/local/tmp/home-grid.xml"
                result = guest("shell", "uiautomator", "dump", path)
                if path not in result:
                    print(f"Waiting for an accessible Android window: {result}", flush=True)
                    return []
                last_tree = guest("shell", "cat", path)
                dom = ET.fromstring(last_tree)
                # A software-rendered cold boot on a hosted runner can briefly stall System UI.
                # Only acknowledge that exact Android dialog, never a Home or game failure.
                if any(n.get("resource-id") == "android:id/alertTitle" and
                       n.get("text") == "System UI isn't responding" for n in dom.iter("node")):
                    wait = next(n for n in dom.iter("node") if n.get("resource-id") == "android:id/aerr_wait")
                    if system_waits >= 3:
                        raise RuntimeError("System UI still unresponsive after three waits")
                    x1, y1, x2, y2 = map(int, re.findall(r"\d+", wait.get("bounds", "")))
                    guest("shell", "input", "tap", str((x1 + x2) // 2), str((y1 + y2) // 2))
                    system_waits += 1
                    print(f"Waiting for System UI after cold boot ({system_waits}/3)", flush=True)
                    return []
                return sorted({int(match[1]) for node in dom.iter("node")
                    for key in ("text", "content-desc")
                    if (match := re.search(rf"AA {kind} (\d+)", node.get(key, "")))})
            for kind in ("Games", "Apps"):
                guest("shell", "am", "start", "-W", "-n", home + "/.HomeActivity", "--ez", "apps", str(kind == "Apps").lower())
                initial = min(wait_for(f"first visible {kind} row", lambda: cards(kind)))
                run("xdotool", "click", "--repeat", "14", "--delay", "70", "5")
                def first_after_scroll():
                    visible = cards(kind)
                    if visible and min(visible) > initial:
                        return min(visible)
                first = wait_for(f"{kind} to scroll down", first_after_scroll, 30)
                run("xdotool", "click", "--repeat", "22", "--delay", "70", "4")
                def original_row():
                    visible = cards(kind)
                    return visible if visible and min(visible) <= initial else None
                restored = min(wait_for(f"{kind} to return to its first row", original_row, 30))
                print(f"{kind}: first visible card {initial} -> {first} -> {restored} via desktop wheel", flush=True)
            guest("shell", "am", "start", "-W", "-n", game + "/.Card0")
            wait_for("fixture in foreground", lambda: any(game in line for line in
                guest("shell", "dumpsys", "window").splitlines() if "mCurrentFocus=" in line))
            # The desktop refreshes the foreground Android package once per second.
            time.sleep(1.5)
            for expected in ("Driving", "On foot"):
                run("xdotool", "key", "--clearmodifiers", "ctrl+shift+p")
                wait_for(f"saved profile {expected}", lambda: json.loads(maps.read_text())[game]["active"] == expected, 7)
            print("Game profiles: On foot -> Driving -> On foot via Ctrl+Shift+P", flush=True)
            counter = run(temp / "test-bin/runtime-install-test", "--exact",
                "controls::tests::live_android_surface_counter_tracks_the_selected_game", "--ignored", "--nocapture",
                env=dict(env, RUSTEMUDROID_TEST_ADB=str(adb), RUSTEMUDROID_TEST_PORT=serial.split("-")[1],
                         RUSTEMUDROID_TEST_AVD=str(avd), RUSTEMUDROID_TEST_PACKAGE=game))
            assert "1 passed; 0 failed" in counter and "counter disabled and cleared" in counter, counter
            print(counter, flush=True)
            native = png_size(subprocess.check_output([adb, "-s", serial, "exec-out", "screencap", "-p"],
                                                      env=env, timeout=35))
            assert native in ((540, 960), (960, 540)), f"Unexpected Android screenshot size {native}"
            check_adaptive_capture(window, desktop_log, state / "captures", native)
        except Exception:
            print("Desktop log tail:", desktop_log.read_text(errors="replace")[-3500:], flush=True)
            if serial:
                names = guest("shell", "cmd", "package", "query-activities", "--brief", "-a", "android.intent.action.MAIN", "-c", "android.intent.category.LAUNCHER")
                print("Fixture launcher activities:", "\n".join(s for s in names.splitlines() if "org.rustemudroid.qa" in s)[:3000], flush=True)
                if last_tree:
                    (report / "home-grid.xml").write_text(last_tree)
                    print("Visible labels:", [(n.get("package"), n.get("text"), n.get("content-desc")) for n in ET.fromstring(last_tree).iter("node") if n.get("text") or n.get("content-desc")][:35], flush=True)
                print(guest("shell", "dumpsys", "activity", "top-resumed")[:1000], flush=True)
                messages = guest("logcat", "-d", "-t", "200", "AndroidRuntime:E", "ActivityManager:W", "*:S")
                print(messages[-3000:], flush=True)
            raise
        finally:
            if serial:
                guest("emu", "kill")
            if client.poll() is None:
                client.terminate()
                client.wait(timeout=15)


if __name__ == "__main__":
    main()
