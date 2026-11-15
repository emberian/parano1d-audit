#!/usr/bin/env python3
import collections
import json
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "pinned")
uses = collections.defaultdict(list)
pattern = re.compile(r'DomainTag::new\(b"([^"]+)"\)')
for path in sorted(root.rglob("*.rs")):
    text = path.read_text()
    for line_number, line in enumerate(text.splitlines(), 1):
        for match in pattern.finditer(line):
            uses[match.group(1)].append(f"{path.relative_to(root)}:{line_number}")

duplicates = {tag: locations for tag, locations in uses.items() if len(locations) > 1}
print(json.dumps({
    "source_root": str(root),
    "distinct_tags": len(uses),
    "total_uses": sum(map(len, uses.values())),
    "duplicate_tag_uses": duplicates,
}, indent=2, sort_keys=True))
