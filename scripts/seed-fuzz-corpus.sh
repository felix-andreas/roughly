#!/usr/bin/env bash
# Seeds the coverage-guided fuzz corpora from the fixture suites: every
# fixture case source (both stacks) plus the shipped stubs, one file per
# case. Run from the repository root before `cargo fuzz run`.
set -euo pipefail
cd "$(dirname "$0")/.."
for target in parse format semantics; do
  mkdir -p "fuzz/corpus/$target"
done
python3 - << 'PY'
import hashlib, pathlib

roots = [pathlib.Path("crates")]
sources = []
for root in roots:
    for test_file in root.rglob("*.test"):
        text = test_file.read_text(errors="replace")
        # A fixture case's source is the lines between its `#----` header and
        # the `#++++` expectation marker.
        case = []
        collecting = False
        for line in text.splitlines():
            if line.startswith("#----"):
                collecting = True
                case = []
            elif line.startswith("#++++"):
                if case:
                    sources.append("\n".join(case) + "\n")
                collecting = False
            elif collecting:
                case.append(line)
    for stub in root.rglob("*.Rtypes"):
        sources.append(stub.read_text(errors="replace"))

for target in ("parse", "format", "semantics"):
    out = pathlib.Path("fuzz/corpus") / target
    for source in sources:
        digest = hashlib.sha1(source.encode()).hexdigest()
        (out / digest).write_text(source)
print(f"seeded {len(sources)} case sources into fuzz/corpus/{{parse,format,semantics}}")
PY
