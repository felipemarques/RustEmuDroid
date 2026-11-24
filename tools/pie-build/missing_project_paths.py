"""Find absent r61 manifest projects explicitly named by a failed build rule."""
import re
import sys
from pathlib import Path


def missing_projects(log, project_paths, checkout):
    projects = sorted(set(project_paths), key=len, reverse=True)
    selected = set()
    for line in log.splitlines():
        if not re.search(r"no such file|does not exist|missing and no known rule|"
                         r"no rule to make target|cannot find|cannot locate", line, re.I):
            continue
        for token in re.findall(r"[a-zA-Z0-9_.+@/-]+/[a-zA-Z0-9_.+@/-]+", line):
            path = token.removeprefix("./")
            for project in projects:
                if path == project or path.startswith(project + "/"):
                    if not (checkout / project / ".git").exists():
                        selected.add(project)
                    break
    return sorted(selected)


if __name__ == "__main__":
    log = Path(sys.argv[1]).read_text(errors="replace")
    manifest_paths = Path(sys.argv[2]).read_text().splitlines()
    checked = set(Path(sys.argv[3]).read_text().splitlines())
    for item in missing_projects(log, manifest_paths, Path.cwd()):
        if item not in checked:
            print(item)
