"""Locate missing Blueprint module declarations in the official r61 manifest."""
import base64
import concurrent.futures
import json
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path


def fetch(url):
    for attempt in range(3):
        try:
            with urllib.request.urlopen(url, timeout=18) as response:
                return response.read()
        except (OSError, TimeoutError) as error:
            if isinstance(error, urllib.error.HTTPError) and error.code < 500 and error.code != 429:
                raise
            if attempt == 2:
                raise
            time.sleep(2 ** attempt)


def declarations(source):
    return re.findall(r'^\s*name\s*:\s*"([^"]+)"', source, re.MULTILINE)


def sparse_modules(name, revision):
    with tempfile.TemporaryDirectory(prefix="pie-blueprint-") as temporary:
        root = Path(temporary)

        def git(*args):
            subprocess.run(["git", "-C", temporary, *args], check=True, timeout=150,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

        git("init", "--quiet")
        git("remote", "add", "origin", "https://android.googlesource.com/" + name)
        git("config", "remote.origin.promisor", "true")
        git("config", "remote.origin.partialclonefilter", "blob:none")
        git("fetch", "--quiet", "--depth=1", "--no-tags", "--filter=blob:none", "origin", revision)
        git("sparse-checkout", "set", "--no-cone", "**/Android.bp")
        git("checkout", "--quiet", "--detach", "FETCH_HEAD")
        modules = set()
        for path in root.rglob("Android.bp"):
            if path.is_file() and not path.is_symlink():
                modules.update(declarations(path.read_text(errors="replace")))
        return sorted(modules)


def read_modules(project):
    name, revision = project
    root = "https://android.googlesource.com/" + name + "/+/" + revision + "/"
    entries = json.loads(fetch(root + "?format=JSON&recursive=1")[5:])["entries"]
    blueprints = [entry["name"] for entry in entries
                  if entry["type"] == "blob" and Path(entry["name"]).name == "Android.bp"]
    if len(blueprints) > 10:
        return sparse_modules(name, revision)
    modules = set()
    for filename in blueprints:
        url = root + urllib.parse.quote(filename, safe="/") + "?format=TEXT"
        source = base64.b64decode(fetch(url)).decode(errors="replace")
        modules.update(declarations(source))
    return sorted(modules)


def score(path, wanted):
    basename = path.rsplit("/", 1)[-1].lower()
    aliases = {basename, re.sub(r"^lib", "", basename)}
    if any(len(word) > 2 and word in item.lower() for item in wanted for word in aliases):
        return 0
    if path.startswith(("system/", "frameworks/", "bootable/", "device/generic/", "tools/")):
        return 1
    if path.startswith("external/") and not path.startswith("external/python/"):
        return 2
    if path.startswith(("hardware/", "test/", "prebuilts/")):
        return 3
    return 4


def providers(log, manifest, cache_file):
    wanted = set(re.findall(r'depends on undefined module "([^"]+)"', log))
    if not wanted:
        return []
    root = ET.parse(manifest).getroot()
    default = root.find("default")
    assert default.attrib["revision"] == "refs/tags/android-9.0.0_r61"
    projects = {}
    for entry in root.findall("project"):
        name = entry.attrib["name"]
        revision = entry.get("revision", default.attrib["revision"])
        path = entry.get("path", name)
        if (Path(path) / ".git").exists():
            continue
        projects[path] = (name, revision)
    cache = json.loads(cache_file.read_text()) if cache_file.exists() else {}
    owners = {}

    def collect(path, modules):
        for name in wanted.intersection(modules):
            owners.setdefault(name, set()).add(path)

    for path in projects:
        if path in cache:
            collect(path, cache[path])
    pending = sorted(projects.keys() - cache.keys(), key=lambda path: (score(path, wanted), path))
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        for start in range(0, len(pending), 12):
            if wanted == owners.keys():
                break
            batch = pending[start:start + 12]
            futures = {pool.submit(read_modules, projects[path]): path for path in batch}
            for future in concurrent.futures.as_completed(futures):
                path = futures[future]
                try:
                    cache[path] = future.result()
                except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
                    print(f"Gitiles index unavailable for {path}: {error}", file=sys.stderr)
                    continue
                collect(path, cache[path])
            cache_file.write_text(json.dumps(cache, sort_keys=True))
            if owners and start >= 36:
                break
    for name, paths in sorted(owners.items()):
        print(f"Official r61 Blueprint declares {name}: {', '.join(sorted(paths))}", file=sys.stderr)
    return sorted({path for paths in owners.values() for path in paths})


if __name__ == "__main__":
    for project in providers(Path(sys.argv[1]).read_text(errors="replace"), sys.argv[2], Path(sys.argv[3])):
        print(project)
