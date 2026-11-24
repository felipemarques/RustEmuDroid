"""Export r61 platform sources without retaining a second copy in Git packfiles."""
import argparse
import concurrent.futures
import shutil
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path


def selected(path):
    if path.startswith("device/"):
        return path == "device/common" or path.startswith("device/generic/")
    if path.startswith("prebuilts/"):
        if any(part in path.split("/") for part in ("darwin-x86", "mips", "arm", "aarch64")):
            return False
        return path not in {"prebuilts/android-emulator", "prebuilts/deqp", "prebuilts/qemu-kernel",
                            "prebuilts/gdb/linux-x86", "prebuilts/gcc/linux-x86/host/x86_64-w64-mingw32-4.8"}
    return True


def export_project(project):
    path, name, revision = project
    directory = Path(path)
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="pie-objects-") as temporary:
        gitdir = ["git", "--git-dir=" + temporary]
        subprocess.run(["git", "init", "--bare", "--quiet", temporary], check=True)
        remote = "https://android.googlesource.com/" + name
        for attempt, delay in enumerate((0, 8, 25, 60, 110)):
            if delay:
                print(f"Retrying {path} after {delay}s", flush=True)
                time.sleep(delay)
            fetched = subprocess.run(gitdir + ["fetch", "--quiet", "--depth=1", "--no-tags", remote, revision])
            if fetched.returncode == 0:
                break
            if attempt == 4:
                fetched.check_returncode()
        commit = subprocess.check_output(gitdir + ["rev-parse", "FETCH_HEAD^{commit}"], text=True).strip()
        subprocess.run(gitdir + ["read-tree", commit], check=True)
        subprocess.run(gitdir + ["--work-tree=" + str(directory.resolve()), "checkout-index",
                                 "--all", "--force"], cwd=directory, check=True)
    return path, commit


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    root = ET.parse(args.manifest).getroot()
    revision = root.find("default").attrib["revision"]
    if revision != "refs/tags/android-9.0.0_r61":
        raise ValueError("Unexpected upstream default revision")
    checked = set((args.report / "projects.txt").read_text().splitlines())
    projects = []
    for item in root.findall("project"):
        name = item.attrib["name"]
        path = item.get("path", name)
        if path not in checked and selected(path):
            if len(item):
                raise ValueError(f"{path} has manifest link rules and must use repo sync")
            projects.append((path, name, item.get("revision", revision)))
    print(f"Exporting {len(projects)} projects in addition to {len(checked)} pinned checkouts", flush=True)
    with (args.report / "projects.txt").open("a") as paths, (args.report / "revisions.txt").open("a") as commits:
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            jobs = {pool.submit(export_project, project): project[0] for project in projects}
            for index, job in enumerate(concurrent.futures.as_completed(jobs), 1):
                path, commit = job.result()
                print(path, file=paths, flush=True)
                print(path, commit, file=commits, flush=True)
                if index % 20 == 0 or index == len(projects):
                    free = shutil.disk_usage(".").free / 2**30
                    print(f"Exported {index}/{len(projects)} projects, {free:.1f} GiB free", flush=True)


if __name__ == "__main__":
    main()
