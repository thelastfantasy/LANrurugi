#!/usr/bin/env python3
"""Converts legacy LANraragi `locales/template/*.po` (gettext) files into the
i18next-shaped JSON resources under `frontend/src/i18n/locales/`, per
research.md #10 (US7, T079).

Legacy files use the msgid itself as the English source string (no opaque
keys), so the JSON keyed by msgid doubles as the i18next English fallback
resource. Entries with an empty msgstr (untranslated in that language) are
omitted entirely so i18next's `fallbackLng: 'en'` naturally supplies the
English text (FR-019) instead of rendering blank.

Usage: python3 convert-po-locales.py <path-to-legacy-locales-template-dir>
"""

import json
import re
import sys
from pathlib import Path

LANGUAGES = [
    "en", "ja", "zh", "zh_Hant", "ko", "fr", "de", "es", "it", "pt", "vi", "id", "nb_NO", "as",
]

LINE_RE = re.compile(r'^(msgid|msgstr)\s+"(.*)"\s*$')


def unescape(s: str) -> str:
    return s.replace('\\n', '\n').replace('\\"', '"').replace('\\\\', '\\')


def parse_po(path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}
    msgid: str | None = None
    with path.open(encoding="utf-8") as f:
        for raw_line in f:
            match = LINE_RE.match(raw_line.strip())
            if not match:
                continue
            kind, value = match.group(1), unescape(match.group(2))
            if kind == "msgid":
                msgid = value
            elif kind == "msgstr" and msgid is not None:
                if msgid and value:
                    entries[msgid] = value
                msgid = None
    return entries


def main() -> None:
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(1)

    src_dir = Path(sys.argv[1])
    out_dir = Path(__file__).resolve().parent.parent / "src" / "i18n" / "locales"
    out_dir.mkdir(parents=True, exist_ok=True)

    for lang in LANGUAGES:
        po_path = src_dir / f"{lang}.po"
        entries = parse_po(po_path)
        out_path = out_dir / f"{lang}.json"
        with out_path.open("w", encoding="utf-8") as f:
            json.dump(entries, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"{lang}: {len(entries)} strings -> {out_path.relative_to(Path.cwd())}")


if __name__ == "__main__":
    main()
