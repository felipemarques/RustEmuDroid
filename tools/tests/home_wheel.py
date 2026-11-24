"""Exercise X11 wheel input against both grids in the packaged client on a disposable CI AVD."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET


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


def build_fixtures(work, sdk):
    tools = max((sdk / "build-tools").glob("[0-9]*"), key=lambda p: tuple(map(int, re.findall(r"\d+", p.name))))
    jar = max(sdk.glob("platforms/android-*/android.jar"), key=lambda p: int(re.search(r"android-(\d+)", str(p)).group(1)))
    key = work / "test.jks"
    run("keytool", "-genkeypair", "-keystore", key, "-storepass", "fixture", "-keypass", "fixture",
        "-alias", "fixture", "-keyalg", "RSA", "-dname", "CN=CI fixture", "-validity", "2")
    for kind in ("Games", "Apps"):
        package = f"org.rustemudroid.qa.{kind.lower()}"
        attrs = {'android:name': 'android.app.Activity', 'android:exported': 'false'}
        manifest = ET.Element("manifest", {"xmlns:android": "http://schemas.android.com/apk/res/android", "package": package})
        ET.SubElement(manifest, "uses-sdk", {"android:minSdkVersion": "26", "android:targetSdkVersion": "32"})
        app = ET.SubElement(manifest, "application", {"android:hasCode": "false", "android:appCategory": "game" if kind == "Games" else "productivity"})
        ET.SubElement(app, "activity", attrs)
        for i in range(40):
            alias = ET.SubElement(app, "activity-alias", {"android:name": f".Card{i}", "android:label": f"AA {kind} {i:02d}",
                "android:targetActivity": "android.app.Activity", "android:exported": "true"})
            intent = ET.SubElement(alias, "intent-filter")
            ET.SubElement(intent, "action", {"android:name": "android.intent.action.MAIN"})
            ET.SubElement(intent, "category", {"android:name": "android.intent.category.LAUNCHER"})
        path = work / "AndroidManifest.xml"
        ET.ElementTree(manifest).write(path, encoding="utf-8")
        unsigned = work / f"{kind}-unsigned.apk"
        aligned = work / f"{kind}-aligned.apk"
        signed = work / f"{kind}.apk"
        run(tools / "aapt", "package", "-f", "-M", path, "-I", jar, "-F", unsigned)
        run(tools / "zipalign", "-f", "4", unsigned, aligned)
        run(tools / "apksigner", "sign", "--ks", key, "--ks-pass", "pass:fixture", "--out", signed, aligned)
        yield signed


def main():
    temp = Path(os.environ["RUNNER_TEMP"])
    app = temp / "rustemudroid"
    state = temp / "profile"
    adb = app / "runtime/platform-tools/adb"
    avd = state / "avd/RustEmuDroid.avd"
    assert avd.is_dir()
    with tempfile.TemporaryDirectory() as tmp:
        fixtures = list(build_fixtures(Path(tmp), Path(os.environ["ANDROID_HOME"])))
        env = {k: v for k, v in os.environ.items() if k not in ("ANDROID_HOME", "ANDROID_SDK_ROOT", "ANDROID_AVD_HOME", "ANDROID_USER_HOME", "WAYLAND_DISPLAY")}
        env["HOME"] = str(temp / "guest-home")
        def adb_run(*args):
            return run(adb, *args, env=env)
        assert adb_run("devices").splitlines() == ["List of devices attached"], "Requires an idle runner"
        # Exercise the real packaged window at phone density within the software renderer's budget.
        config = avd / "config.ini"
        values = {"hw.lcd.width": 540, "hw.lcd.height": 960, "hw.lcd.density": 210}
        settings = config.read_text()
        for key, value in values.items():
            settings, count = re.subn(rf"^{re.escape(key)}=.+$", f"{key}={value}", settings, flags=re.M)
            assert count == 1, key
        config.write_text(settings)
        client = subprocess.Popen([app / "rustemudroid", "--lang", "en-us", "--state-dir", state, "ui", "--gpu", "swiftshader"], env=env)
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
            geometry = dict(line.split("=", 1) for line in
                run("xdotool", "getwindowgeometry", "--shell", window).splitlines() if "=" in line)
            run("xdotool", "mousemove", "--window", window,
                str(int(geometry["WIDTH"]) // 2), str(int(geometry["HEIGHT"]) * 3 // 5))
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
        except Exception:
            if serial:
                report = temp / "boot-report"
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
