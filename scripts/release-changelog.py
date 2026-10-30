#!/usr/bin/env python3
"""Release notes for one tag, Chinese and English, generated from conventional commits.

Why this file exists: GitHub Releases are the changelog the app itself reads (`GET
/api/version?since=` in `crates/lanrurugi-api/src/version.rs` returns each release's `body`), so
"the changelog" is the release body. Writing it here keeps commit history as the single source of
truth instead of a second, hand-maintained file.

Chinese comes straight from the commit subjects. English is a DeepSeek translation of those same
lines — `DEEPSEEK_API_KEY` in the environment enables it; without a key (or when the call fails)
the release still publishes, with the Chinese section only and a note saying why.

Usage:
    scripts/release-changelog.py                     # previous tag..HEAD
    scripts/release-changelog.py --from v0.2.0 --to v0.3.0
    scripts/release-changelog.py --no-translate      # Chinese only
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass

#: Type prefix -> (Chinese heading, English heading), in display order. Unknown types fall into the
#: last group so nothing is silently dropped.
GROUPS: list[tuple[tuple[str, ...], str, str]] = [
    (("feat",), "新功能", "Features"),
    (("fix",), "问题修复", "Bug Fixes"),
    (("perf",), "性能优化", "Performance"),
    (("refactor",), "重构", "Refactoring"),
    (("docs",), "文档", "Documentation"),
    (("test",), "测试", "Tests"),
    (("ci", "build"), "构建与 CI", "Build & CI"),
    (("chore",), "其它", "Chores"),
]
OTHER = ("其它", "Other")

#: One group larger than this is truncated with a "…还有 N 条" line: a 300-commit first release
#: would otherwise bury the modal it renders in.
DEFAULT_MAX_PER_GROUP = 25

#: The repository whose release this is, for per-commit links. Overridable for forks.
DEFAULT_REPO = "thelastfantasy/LANrurugi"

DEEPSEEK_MODEL = os.environ.get("CHANGELOG_MODEL", "deepseek-v4-flash")
DEEPSEEK_BASE_URL = os.environ.get("LANRURUGI_DEEPSEEK_BASE_URL", "https://api.deepseek.com")


@dataclass
class Entry:
    sha: str
    type: str
    scope: str
    subject: str


def run(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, capture_output=True, text=True, check=check)


def repo_slug() -> str:
    return os.environ.get("GITHUB_REPOSITORY") or DEFAULT_REPO


def tags_by_version() -> list[str]:
    """Release tags reachable from HEAD, newest version first. `--merged HEAD` keeps a tag from a
    side branch out of the range arithmetic."""
    out = run("git", "tag", "--sort=-v:refname", "--merged", "HEAD", "--list", "v*").stdout
    return [line.strip() for line in out.splitlines() if line.strip()]


def exact_tag_at_head() -> str | None:
    result = run("git", "describe", "--tags", "--exact-match", "HEAD", check=False)
    return result.stdout.strip() or None


def previous_tag() -> str | None:
    """The tag this release comes after: the highest release tag below the one at HEAD. `None` when
    HEAD is not tagged (local run) — then the newest tag is the previous one."""
    tags = tags_by_version()
    if not tags:
        return None
    current = exact_tag_at_head()
    if current in tags:
        index = tags.index(current)
        return tags[index + 1] if index + 1 < len(tags) else None
    return tags[0]


def parse_subject(shasubject: str) -> tuple[str, str, str] | None:
    """Split `type(scope)!: subject` into its parts; `None` when it is not conventional."""
    head, _, subject = shasubject.partition(":")
    if not subject:
        return None
    head = head.strip()
    bang = head.endswith("!")
    if bang:
        head = head[:-1]
    type_, scope = head, ""
    if "(" in head and head.endswith(")"):
        type_, _, scope = head.partition("(")
        scope = scope[:-1]
    type_ = type_.strip().lower()
    if not type_.isalnum():
        return None
    return type_, scope.strip(), subject.strip()


def collect(from_ref: str | None, to_ref: str, max_commits: int) -> list[Entry]:
    rev = f"{from_ref}..{to_ref}" if from_ref else to_ref
    # `%x1f`/`%x1e` rather than newlines: subjects are one line, bodies are not.
    out = run(
        "git",
        "log",
        "--no-merges",
        f"--max-count={max_commits}",
        "--format=%H%x1f%s%x1e",
        rev,
    ).stdout
    entries: list[Entry] = []
    for record in out.split("\x1e"):
        record = record.strip("\n")
        if not record:
            continue
        sha, _, subject = record.partition("\x1f")
        parsed = parse_subject(subject)
        if parsed is None:
            entries.append(Entry(sha=sha, type="other", scope="", subject=subject.strip()))
            continue
        type_, scope, text = parsed
        entries.append(Entry(sha=sha, type=type_, scope=scope, subject=text))
    return entries


def grouped(entries: list[Entry]) -> dict[str, list[Entry]]:
    order = {t: i for i, (types, _, _) in enumerate(GROUPS) for t in types}
    buckets: dict[str, list[Entry]] = {}
    for entry in entries:
        buckets.setdefault(entry.type if entry.type in order else "other", []).append(entry)
    return dict(sorted(buckets.items(), key=lambda kv: order.get(kv[0], len(GROUPS))))


def heading(type_: str) -> tuple[str, str]:
    for types, zh, en in GROUPS:
        if type_ in types:
            return zh, en
    return OTHER


def line(entry: Entry, repo: str) -> str:
    scope = f"**{entry.scope}**: " if entry.scope else ""
    return f"- {scope}{entry.subject} ([{entry.sha[:7]}](https://github.com/{repo}/commit/{entry.sha}))"


def render_section(buckets: dict[str, list[Entry]], repo: str, language: int, max_per_group: int) -> list[str]:
    lines: list[str] = []
    for type_, entries in buckets.items():
        zh, en = heading(type_)
        title = (zh, en)[language]
        shown = entries[:max_per_group]
        lines.append(f"### {title}")
        lines.extend(line(entry, repo) for entry in shown)
        if len(entries) > len(shown):
            more = len(entries) - len(shown)
            lines.append(
                f"- _…另外 {more} 条_"
                if language == 0
                else f"- _…and {more} more_"
            )
        lines.append("")
    return lines


def translate(subjects: list[str]) -> list[str] | None:
    """Translate commit subjects to English, or `None` when no key is configured / the call fails.

    One request for the whole release: a per-subject loop would be dozens of round trips for what
    is a few hundred tokens of text.
    """
    key = os.environ.get("DEEPSEEK_API_KEY", "").strip()
    if not key or not subjects:
        return None
    payload = {
        "model": DEEPSEEK_MODEL,
        "temperature": 0,
        "response_format": {"type": "json_object"},
        "messages": [
            {
                "role": "system",
                "content": (
                    "You translate changelog entries from Chinese to English for a self-hosted "
                    "manga library manager. Reply with a JSON object {\"items\": [...]} whose "
                    "array has exactly the same length and order as the input. Keep product and "
                    "feature names, abbreviations and technical terms (WebP, Redis, CodeQL, "
                    "Deno, pnpm, ONNX) untranslated. Each line must be a short imperative or noun "
                    "phrase, no trailing period, no quotes of its own."
                ),
            },
            {"role": "user", "content": json.dumps({"items": subjects}, ensure_ascii=False)},
        ],
    }
    request = urllib.request.Request(
        f"{DEEPSEEK_BASE_URL.rstrip('/')}/chat/completions",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            body = json.loads(response.read().decode())
        content = body["choices"][0]["message"]["content"]
        items = json.loads(content)["items"]
        if not isinstance(items, list) or len(items) != len(subjects):
            raise ValueError(f"expected {len(subjects)} translations, got {len(items)}")
        return [str(item) for item in items]
    except (urllib.error.URLError, KeyError, IndexError, ValueError, json.JSONDecodeError) as error:
        print(f"::warning::English changelog skipped: {error}", file=sys.stderr)
        return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from", dest="from_ref", default=None)
    parser.add_argument("--to", dest="to_ref", default="HEAD")
    parser.add_argument("--output", default="release-notes.md")
    parser.add_argument("--max-per-group", type=int, default=DEFAULT_MAX_PER_GROUP)
    parser.add_argument("--max-commits", type=int, default=400)
    parser.add_argument("--no-translate", action="store_true")
    args = parser.parse_args()

    from_ref = args.from_ref or previous_tag()
    entries = collect(from_ref, args.to_ref, args.max_commits)
    repo = repo_slug()

    lines: list[str] = []
    if from_ref is None:
        lines.append("## 中文")
        lines.append("LANrurugi 首个版本。 / _First release._")
        lines.append("")
    else:
        lines.append(f"## 中文\n")
    lines.extend(render_section(grouped(entries), repo, language=0, max_per_group=args.max_per_group))

    english: list[str] | None = None
    if not args.no_translate:
        buckets = grouped(entries)
        english = translate([entry.subject for bucket in buckets.values() for entry in bucket])
    lines.append("## English")
    if english is None:
        lines.append(
            "_English notes were not generated for this release (no `DEEPSEEK_API_KEY`, or the "
            "translation call failed); the Chinese section above is authoritative._"
        )
    else:
        lines.extend(render_section_translated(grouped(entries), repo, english, args.max_per_group))

    text = "\n".join(lines).rstrip() + "\n"
    with open(args.output, "w", encoding="utf-8") as handle:
        handle.write(text)
    print(f"wrote {args.output} ({len(entries)} commits, english={'yes' if english else 'no'})")
    return 0


def render_section_translated(
    buckets: dict[str, list[Entry]], repo: str, translated: list[str], max_per_group: int
) -> list[str]:
    """The English section: same sections, same order, with the translated subjects spliced in.

    `translated` arrives as one flat list in `buckets` order, matching `translate`'s single call.
    """
    lines: list[str] = []
    cursor = 0
    for type_, entries in buckets.items():
        _, en = heading(type_)
        shown = entries[:max_per_group]
        lines.append(f"### {en}")
        for offset, entry in enumerate(shown):
            scope = f"**{entry.scope}**: " if entry.scope else ""
            subject = translated[cursor + offset]
            lines.append(f"- {scope}{subject} ([{entry.sha[:7]}](https://github.com/{repo}/commit/{entry.sha}))")
        if len(entries) > len(shown):
            lines.append(f"- _…and {len(entries) - len(shown)} more_")
        lines.append("")
        cursor += len(entries)
    return lines


if __name__ == "__main__":
    sys.exit(main())
