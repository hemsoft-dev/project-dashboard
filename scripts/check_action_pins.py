#!/usr/bin/env python3
"""Reject workflow actions that aren't pinned to a full commit (#103).

Every `uses:` in .github/workflows/*.yml and *.yaml must name a full 40-character commit SHA followed by the release
it is, as in `uses: actions/checkout@<sha> # v4.4.0`. A tag or branch such as `@v4` can move upstream and change the
code a job runs without a change here. Local actions (`./path`) and digest-pinned containers (`docker://...@sha256:`)
are immutable already and pass.

    python scripts/check_action_pins.py                  # offline: every reference is pinned and labelled
    python scripts/check_action_pins.py --verify         # also ask GitHub that each label's tag is that commit (gh CLI)

Exits 0 when every reference passes, 1 when any fails, 2 on a usage or read error. docs/action-pins.md says how to
update a pin.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import yaml  # PyYAML: python3-yaml on Ubuntu, `pip install pyyaml` elsewhere
except ImportError:  # pragma: no cover - exercised by hand
    print("error   check_action_pins.py needs PyYAML (python3-yaml, or pip install pyyaml)", file=sys.stderr)
    sys.exit(2)

PINNED = re.compile(r"^(?P<repo>[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)(?P<path>/[^@]+)?@(?P<sha>[0-9a-f]{40})$")
VERSION = re.compile(r"^v\d+(?:\.\d+){0,2}$")
DIGEST = re.compile(r"^docker://[^@\s]+@sha256:[0-9a-f]{64}$")

# Intentional mutable references, as "path:line": reason. Keep it empty unless an action can't be pinned; a reviewer
# must see the reason in the diff that adds one.
EXCEPTIONS: dict[str, str] = {}


@dataclass(frozen=True)
class Use:
    file: Path
    line: int
    ref: str
    comment: str | None

    @property
    def where(self) -> str:
        return f"{self.file.as_posix()}:{self.line}"


UNPARSED = "<unparsed>"
COMMENT = re.compile(r"#\s*(?P<comment>\S.*?)\s*$")


class WorkflowError(Exception):
    """A workflow file that doesn't parse as YAML."""


def find_uses(path: Path) -> list[Use]:
    """Every `uses` key in the file, found by parsing it as YAML.

    The parser decodes keys the way GitHub does, so a quoted, escaped or flow-mapping spelling (`{"\\u0075ses": ...}`)
    is still a `uses` key. An earlier line-by-line reader missed such spellings (SFL on #109). The raw line is read only
    for the `# vX.Y.Z` comment after the value, which YAML drops.
    """
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    try:
        documents = list(yaml.compose_all(text, Loader=yaml.SafeLoader))
    except yaml.YAMLError as err:
        raise WorkflowError(f"{path.as_posix()}: not valid YAML: {err}") from err
    out: list[Use] = []
    seen: set[int] = set()

    def walk(node) -> None:
        if node is None or id(node) in seen:  # an alias is the anchored node itself
            return
        seen.add(id(node))
        if isinstance(node, yaml.MappingNode):
            for key, value in node.value:
                if isinstance(key, yaml.ScalarNode) and key.value == "uses":
                    out.append(use_of(value))
                walk(key)
                walk(value)
        elif isinstance(node, yaml.SequenceNode):
            for item in node.value:
                walk(item)

    def use_of(value) -> Use:
        line = value.start_mark.line + 1
        if not isinstance(value, yaml.ScalarNode) or value.style in ("|", ">"):
            # A list, a mapping or a block scalar isn't an action reference GitHub accepts; fail rather than guess.
            return Use(path, line, UNPARSED, None)
        end = value.end_mark
        rest = lines[end.line][end.column:] if end.line < len(lines) else ""
        m = COMMENT.search(rest)
        return Use(path, line, value.value.strip(), m["comment"] if m else None)

    for document in documents:
        walk(document)
    return out

def problem(use: Use) -> str | None:
    """Why a reference fails, or None when it is immutable and labelled."""
    if use.ref == UNPARSED:
        return "not a plain action reference; write it as `uses: owner/repo@<sha> # vX.Y.Z`"
    if use.ref.startswith("./"):
        return None
    if use.ref.startswith("docker://"):
        return None if DIGEST.match(use.ref) else "container image is not pinned to a sha256 digest"
    m = PINNED.match(use.ref)
    if not m:
        if "@" not in use.ref:
            return "no ref: pin a full commit SHA"
        ref = use.ref.rsplit("@", 1)[1]
        if re.fullmatch(r"[0-9a-fA-F]{7,40}", ref):
            return f"short or uppercase SHA {ref!r}: pin the full 40-character lowercase commit"
        return f"mutable ref {ref!r}: pin the full commit SHA it resolves to"
    if not use.comment or not VERSION.match(use.comment.split()[0]):
        return "pinned SHA has no `# vX.Y.Z` comment naming its release"
    return None


def workflow_files(root: Path) -> list[Path]:
    folder = root / ".github" / "workflows"
    return sorted([*folder.glob("*.yml"), *folder.glob("*.yaml")])


def gh_json(endpoint: str):
    run = subprocess.run(["gh", "api", endpoint], capture_output=True, text=True, check=False)
    if run.returncode != 0:
        raise RuntimeError(f"gh api {endpoint}: {run.stderr.strip()}")
    return json.loads(run.stdout)


def tag_commit(repo: str, tag: str) -> str:
    obj = gh_json(f"repos/{repo}/git/ref/tags/{tag}")["object"]
    while obj["type"] == "tag":  # an annotated tag points at a tag object first
        obj = gh_json(f"repos/{repo}/git/tags/{obj['sha']}")["object"]
    return obj["sha"]


def verify(use: Use) -> str | None:
    """Ask GitHub whether the release the comment names is the pinned commit."""
    m = PINNED.match(use.ref)
    if not m:
        return None
    tag = use.comment.split()[0]
    try:
        commit = tag_commit(m["repo"], tag)
    except RuntimeError as err:
        return f"couldn't read {m['repo']} {tag}: {err}"
    if commit != m["sha"]:
        return f"{m['repo']} {tag} is {commit}, not the pinned {m['sha']}"
    return None


def check(root: Path, files: list[Path], online: bool, out=sys.stdout) -> int:
    failures = 0
    count = 0
    for path in files:
        try:
            uses = find_uses(path)
        except WorkflowError as err:
            failures += 1
            print(f"error   {err}", file=out)
            continue
        for use in uses:
            count += 1
            shown = Use(path.relative_to(root) if path.is_relative_to(root) else path, use.line, use.ref, use.comment)
            reason = problem(use)
            if reason is None and online:
                reason = verify(use)
            if reason is not None and shown.where in EXCEPTIONS:
                print(f"allowed {shown.where} {use.ref}: {EXCEPTIONS[shown.where]}", file=out)
                continue
            if reason is not None:
                failures += 1
                print(f"error   {shown.where} {use.ref}: {reason}", file=out)
            elif online:
                print(f"ok      {shown.where} {use.ref} # {use.comment}", file=out)
    if count == 0:
        print("error   no `uses:` references found; is this the repository root?", file=out)
        return 1
    print(f"{count} references, {failures} failing{' (checked against GitHub)' if online else ''}", file=out)
    return 1 if failures else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("files", nargs="*", type=Path, help="workflow files (default: .github/workflows/*.yml)")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--verify", action="store_true", help="check each comment's tag against GitHub with gh")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    files = [f.resolve() for f in args.files] or workflow_files(root)
    try:
        return check(root, files, args.verify)
    except OSError as err:
        print(f"error   {err}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
