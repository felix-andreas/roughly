#!/usr/bin/env python3
"""Convert tree-sitter-r test corpus files into the project fixture format.

Reads every ``corpus/tree-sitter-r/*.txt`` file, splits it into the tree-sitter
test sections (name / source / expected S-expression), and emits one project
fixture file per corpus file at ``crates/syntax/tests/tsr/<stem>.R.test``. The
original expected S-expression of each section is written alongside to
``corpus/tree-sitter-r-expected/<stem>/<slug>.sexp`` (gitignored) so the parser's
output can later be adjudicated against tree-sitter's.

Tree-sitter corpus grammar (with the robustness rules this converter relies on):

  * A section header is a line of only ``=`` (>= 3), then one or more name /
    attribute lines (none of which is an all-``=`` line), then another line of
    only ``=`` (>= 3). Attribute lines start with ``:`` (e.g. ``:skip``,
    ``:error``, ``:language(...)``).
  * The source code follows the header, then a divider line of only ``-``
    (>= 3), then the expected S-expression, which runs until the next header.
  * R source can itself contain lines of ``-`` or ``=``. We disambiguate with two
    facts that always hold for real corpora: a header is only a header when a
    closing ``=`` fence follows the name block, and the expected S-expression
    never contains a standalone all-``-`` line, so the *last* ``-`` divider in a
    section body is the true source/expected split.

The expectation body we emit is intentionally empty (a single blank line): these
fixtures stage tree-sitter's cases for the hand-written parser; the parser's
golden trees are filled in later. Sections marked ``:skip`` are dropped. Sections
whose expected S-expression contains ``(ERROR`` or ``(MISSING`` (recovery tests)
are emitted with a ``_ts_error`` case-name suffix.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_DIR = REPO_ROOT / "corpus" / "tree-sitter-r"
FIXTURE_DIR = REPO_ROOT / "crates" / "syntax" / "tests" / "tsr"
EXPECTED_DIR = REPO_ROOT / "corpus" / "tree-sitter-r-expected"


def is_fence(line: str, char: str) -> bool:
    stripped = line.rstrip()
    return len(stripped) >= 3 and all(character == char for character in stripped)


def is_equals(line: str) -> bool:
    return is_fence(line, "=")


def is_dash(line: str) -> bool:
    return is_fence(line, "-")


def try_header(lines: list[str], index: int) -> tuple[int, list[str]] | None:
    """If a section header opens at ``lines[index]``, return the index of its
    closing ``=`` fence and the header content lines between the fences."""
    if index >= len(lines) or not is_equals(lines[index]):
        return None
    cursor = index + 1
    content: list[str] = []
    while cursor < len(lines) and not is_equals(lines[cursor]):
        content.append(lines[cursor])
        cursor += 1
    if cursor >= len(lines):
        return None  # no closing fence: not a header
    if not any(line.strip() for line in content):
        return None  # empty header block: not a header
    return cursor, content


def slugify(name: str) -> str:
    slug = "".join(character.lower() if character.isalnum() else "_" for character in name)
    slug = slug.strip("_")
    while "__" in slug:
        slug = slug.replace("__", "_")
    return slug or "case"


def strip_blank_edges(lines: list[str]) -> list[str]:
    start = 0
    end = len(lines)
    while start < end and not lines[start].strip():
        start += 1
    while end > start and not lines[end - 1].strip():
        end -= 1
    return lines[start:end]


def parse_sections(text: str) -> list[tuple[str, list[str], list[str], list[str]]]:
    """Return ``(name, attributes, source_lines, expected_lines)`` per section."""
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    sections: list[tuple[str, list[str], list[str], list[str]]] = []

    index = 0
    while index < len(lines):
        header = try_header(lines, index)
        if header is None:
            index += 1  # stray content before the first header (or a blank line)
            continue
        close_index, content = header
        name = " ".join(
            line.strip() for line in content if not line.strip().startswith(":")
        ).strip()
        attributes = [line.strip() for line in content if line.strip().startswith(":")]

        body_start = close_index + 1
        scan = body_start
        next_header = len(lines)
        while scan < len(lines):
            if try_header(lines, scan) is not None:
                next_header = scan
                break
            scan += 1
        body = lines[body_start:next_header]

        dash_positions = [position for position, line in enumerate(body) if is_dash(line)]
        if dash_positions:
            divider = dash_positions[-1]
            source = body[:divider]
            expected = body[divider + 1:]
        else:
            source = body
            expected = []

        sections.append((name, attributes, strip_blank_edges(source), strip_blank_edges(expected)))
        index = next_header

    return sections


def convert_file(path: Path) -> tuple[int, int]:
    """Convert one corpus file. Returns ``(emitted, skipped)`` section counts."""
    stem = path.stem
    text = path.read_text(encoding="utf-8", errors="replace")
    sections = parse_sections(text)

    fixture_lines: list[str] = [f"#==== {stem}", ""]
    expected_stem_dir = EXPECTED_DIR / stem
    used_slugs: dict[str, int] = {}
    emitted = 0
    skipped = 0
    collisions: list[str] = []

    for name, attributes, source, expected in sections:
        if any(attribute.startswith(":skip") for attribute in attributes):
            skipped += 1
            continue

        expected_text = "\n".join(expected)
        is_error = (
            "(ERROR" in expected_text
            or "(MISSING" in expected_text
            or any(attribute.startswith(":error") for attribute in attributes)
        )

        slug = slugify(name)
        if is_error:
            slug = f"{slug}_ts_error"
        count = used_slugs.get(slug, 0) + 1
        used_slugs[slug] = count
        if count > 1:
            slug = f"{slug}_{count}"

        for source_line in source:
            if source_line.startswith(("#====", "#----", "#++++")):
                collisions.append(f"{stem}/{slug}: {source_line!r}")

        fixture_lines.append(f"#---- {slug}")
        fixture_lines.extend(source)
        fixture_lines.append("#++++")
        fixture_lines.append("")

        expected_stem_dir.mkdir(parents=True, exist_ok=True)
        (expected_stem_dir / f"{slug}.sexp").write_text(
            expected_text + "\n", encoding="utf-8"
        )
        emitted += 1

    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    (FIXTURE_DIR / f"{stem}.R.test").write_text("\n".join(fixture_lines) + "\n", encoding="utf-8")

    if collisions:
        print(f"  WARNING: {len(collisions)} source line(s) collide with fixture directives:")
        for collision in collisions:
            print(f"    {collision}")

    return emitted, skipped


def main() -> int:
    if not CORPUS_DIR.is_dir():
        print(f"corpus not found at {CORPUS_DIR} (run scripts/fetch-corpus.sh first)", file=sys.stderr)
        return 1

    corpus_files = sorted(CORPUS_DIR.glob("*.txt"))
    if not corpus_files:
        print(f"no *.txt files under {CORPUS_DIR}", file=sys.stderr)
        return 1

    total_emitted = 0
    total_skipped = 0
    for path in corpus_files:
        emitted, skipped = convert_file(path)
        total_emitted += emitted
        total_skipped += skipped
        print(f"{path.name}: {emitted} emitted, {skipped} skipped")

    print(
        f"\nconverted {len(corpus_files)} file(s): "
        f"{total_emitted + total_skipped} sections total, "
        f"{total_emitted} emitted, {total_skipped} skipped"
    )
    print(f"fixtures -> {FIXTURE_DIR.relative_to(REPO_ROOT)}")
    print(f"expected sexps -> {EXPECTED_DIR.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
