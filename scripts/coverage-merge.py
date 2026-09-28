#!/usr/bin/env python3
"""Combine line coverage from several platforms into one number.

Each platform's report (lcov, from cargo llvm-cov) sees only the code built
there: Linux never compiles the macOS-only code and the other way around. A
line counts once, and is covered when any platform's tests ran it. Paths are
made relative to the repository, since each runner checks out elsewhere.

Usage: coverage-merge.py [--fail-under PCT] [--summary FILE] LABEL=LCOV ...
"""
import argparse
import re
import sys
from collections import defaultdict


def relative(path):
    """/home/runner/work/PkgDeck/PkgDeck/crates/x.rs -> crates/x.rs"""
    match = re.search(r"(?:^|/)(crates/.*)$", path)
    return match.group(1) if match else path


def read(path):
    """{file: {line: hits}} from the DA records of an lcov report."""
    lines = defaultdict(dict)
    current = None
    with open(path, encoding="utf-8") as report:
        for record in report:
            record = record.strip()
            if record.startswith("SF:"):
                current = relative(record[3:])
            elif record.startswith("DA:") and current is not None:
                number, hits = record[3:].split(",")[:2]
                line = int(number)
                lines[current][line] = max(lines[current].get(line, 0), int(hits))
            elif record == "end_of_record":
                current = None
    return lines


def totals(lines):
    found = sum(len(by_line) for by_line in lines.values())
    hit = sum(1 for by_line in lines.values() for hits in by_line.values() if hits > 0)
    return found, hit


def percent(hit, found):
    return 100.0 * hit / found if found else 100.0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("reports", nargs="+", metavar="LABEL=LCOV")
    parser.add_argument("--fail-under", type=float, default=None)
    parser.add_argument("--summary", default=None, help="append a Markdown table here")
    args = parser.parse_args()

    combined = defaultdict(dict)
    rows = []
    for spec in args.reports:
        label, _, path = spec.partition("=")
        if not path:
            parser.error(f"expected LABEL=LCOV, got {spec!r}")
        lines = read(path)
        found, hit = totals(lines)
        rows.append((label, found, hit))
        for file, by_line in lines.items():
            for line, hits in by_line.items():
                combined[file][line] = max(combined[file].get(line, 0), hits)
    found, hit = totals(combined)
    rows.append(("Combined", found, hit))

    table = ["| Coverage | Lines | Covered | Percent |", "| --- | ---: | ---: | ---: |"]
    table += [f"| {label} | {f} | {h} | {percent(h, f):.2f}% |" for label, f, h in rows]
    print("\n".join(table))
    if args.summary:
        with open(args.summary, "a", encoding="utf-8") as summary:
            summary.write("\n".join(table) + "\n")

    total = percent(hit, found)
    if args.fail_under is not None and total < args.fail_under:
        print(f"Combined line coverage {total:.2f}% is under {args.fail_under}%", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
