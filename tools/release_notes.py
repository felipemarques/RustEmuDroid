"""Prepare repository-relative links in a release note for the GitHub Release page."""

import argparse
import re
from pathlib import Path
from urllib.parse import quote, urljoin, urlsplit


def render(markdown, notes_path, repository_url, revision):
    root = f"{repository_url.rstrip('/')}/blob/{quote(revision, safe='')}/"
    base = root + quote(notes_path.as_posix())

    def resolve(match):
        target = match["target"]
        bracketed = target.startswith("<")
        url = target[1:-1] if bracketed else target
        if url.startswith(("#", "//", "?")) or urlsplit(url).scheme:
            return match[0]
        encoded = quote(url, safe="/%#?&=:+@!$,;-._~")
        absolute = urljoin(root, encoded[1:]) if encoded.startswith("/") else urljoin(base, encoded)
        if not absolute.startswith(root):
            raise ValueError(f"Relative link points outside the repository: {url}")
        return match["prefix"] + (f"<{absolute}>" if bracketed else absolute)

    # Release notes use inline links and may also contain reference definitions and code examples.
    markup = re.compile(
        r"(?P<code>(?P<ticks>`+)(?!`).*?(?P=ticks)(?!`))|"
        r"(?P<prefix>(?<!\\)\]\([ \t]*|^[ ]{0,3}\[[^\]\n]+\]:[ \t]*)"
        r"(?P<target><[^>\n]+>|[^\s()<>]+)"
    )
    output = []
    fence = None
    for line in markdown.splitlines(keepends=True):
        marker = re.match(r"^[ ]{0,3}(`{3,}|~{3,})(.*)$", line)
        if fence:
            output.append(line)
            if marker and marker[1][0] == fence[0] and len(marker[1]) >= len(fence) and not marker[2].strip():
                fence = None
        elif marker:
            fence = marker[1]
            output.append(line)
        elif line.startswith(("    ", "\t")):
            output.append(line)
        else:
            output.append(markup.sub(lambda m: m[0] if m["code"] else resolve(m), line))
    return "".join(output)


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("notes", type=Path, help="release note path relative to the checkout root")
    cli.add_argument("repository_url")
    cli.add_argument("revision", help="tag or full commit SHA")
    cli.add_argument("output", type=Path)
    args = cli.parse_args()
    text = render(args.notes.read_text(encoding="utf-8"), args.notes, args.repository_url, args.revision)
    args.output.write_text(text, encoding="utf-8")
