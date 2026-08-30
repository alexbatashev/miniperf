#!/usr/bin/env python3
"""Fail when platform-conditional compilation leaks out of libprof.

`#[cfg(target_os = ...)]` and `#[cfg_attr(..., target_arch = ...)]` make every
target compile a different program, so a change that is green on one platform
routinely breaks another. Only libprof is allowed to select code at compile
time; the CLI tier branches at runtime over capability objects instead.

`cfg!(target_os = ...)` is deliberately not reported: both of its branches
compile on every target, and it is the pattern this guard pushes callers to.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUARDED = ("mperf", "mperf-gui", "store", "mperf-data")
ALLOWLIST = ROOT / "utils" / "platform-cfg-allowlist.txt"
# `#!` too: an inner attribute gates a whole module just as effectively.
ATTRIBUTE = re.compile(r"#!?\[\s*cfg(_attr)?\s*\(")
# `unix` and `windows` are the same leak spelled shorter, and target_family,
# target_env, target_vendor and target_pointer_width all pick a platform.
PLATFORM = re.compile(
    r"\b(unix|windows|target_(os|arch|family|env|vendor|pointer_width))\b"
)


def attributes(source):
    """Yield `(line, text)` for every `cfg`/`cfg_attr` attribute in `source`.

    Attributes are matched by balancing parentheses rather than per line: a
    multi-line `#[cfg(all(\\n    target_os = "linux", ...))]` hides the
    platform predicate from any single-line pattern.
    """
    for match in ATTRIBUTE.finditer(source):
        depth = 0
        for end in range(match.end() - 1, len(source)):
            if source[end] == "(":
                depth += 1
            elif source[end] == ")":
                depth -= 1
                if depth == 0:
                    break
        else:
            continue
        yield source.count("\n", 0, match.start()) + 1, source[match.start() : end + 1]


def main():
    allowed = {
        line.split("#", 1)[0].strip()
        for line in ALLOWLIST.read_text().splitlines()
        if line.split("#", 1)[0].strip()
    }
    hits = []
    for crate in GUARDED:
        for path in sorted((ROOT / crate).rglob("*.rs")):
            relative = path.relative_to(ROOT).as_posix()
            if relative in allowed:
                continue
            source = path.read_text()
            hits += [
                f"{relative}:{line}: {' '.join(text.split())}"
                for line, text in attributes(source)
                if PLATFORM.search(text)
            ]

    stale = sorted(entry for entry in allowed if not (ROOT / entry).is_file())
    for entry in stale:
        print(f"allowlisted file no longer exists: {entry}", file=sys.stderr)

    if hits:
        print(
            f"{len(hits)} platform-conditional attribute(s) outside libprof:\n",
            file=sys.stderr,
        )
        print("\n".join(hits), file=sys.stderr)
        print(
            "\nBranch at runtime over a libprof capability instead, or move the "
            "code into libprof. See CONTRIBUTING.md.",
            file=sys.stderr,
        )
    return 1 if hits or stale else 0


if __name__ == "__main__":
    sys.exit(main())
