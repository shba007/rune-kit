"""Extract release notes from the matching CHANGELOG section."""
import argparse
import re
from pathlib import Path


def extract(text, version):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version):
        raise ValueError("Invalid version")
    matches = list(re.finditer(r"^## (.+)$", text, re.M))
    found = [(i,m) for i,m in enumerate(matches) if re.match(r"\[?"+re.escape(version)+r"(?:\]|\s|$)",m[1])]
    if len(found) != 1:
        raise ValueError("Missing or duplicate version section: " + version)
    i,m = found[0]
    body = text[m.end():matches[i+1].start() if i+1<len(matches) else len(text)].strip()
    if not re.search(r"[A-Za-z]{3}", body) or re.search(r"\b(TODO|TBD|placeholder|unreleased|routine release update|nothing yet|coming soon)\b",body,re.I):
        raise ValueError("Empty or placeholder release notes")
    return body + "\n"


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("version")
    p.add_argument("--output")
    a = p.parse_args()
    try:
        notes = extract(Path("CHANGELOG.md").read_text(encoding="utf-8"),a.version)
    except ValueError as error:
        p.exit(1,str(error)+"\n")
    if a.output:
        Path(a.output).write_text(notes,encoding="utf-8")
    else:
        print(notes,end="")
