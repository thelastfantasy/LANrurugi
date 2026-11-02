// Faithful TypeScript port of legacy's "Filename Parsing" plugin
// (`~/LANraragi/lib/LANraragi/Plugin/Metadata/RegexParse.pm`) — pure local string/regex logic
// over the archive's own filename, no network access at all, so unlike the third-party metadata
// scrapers this one is in scope to port directly (see project decision on plugin scope).
//
// Declares no permissions at all (no `net`, `read`, or `write`) since it only operates on the
// filename string the host already passes in via `args.file_path`.

const PLUGIN_TAG_NS = "parsed:";

const COMMON_EXTRANEOUS_VALUES = new Set([
  "uncensored",
  "decensored",
  "ongoing",
  "pixiv",
  "twitter",
  "fanbox",
  "cosplay",
  "digital",
]);

const DEFAULT_REGEX =
  String.raw`(\((?<event>[^([]+)\))?\s*(\[(?<artist>[^\]]+)\])?\s*(?<title>[^([]+)\s*(\((?<series>[^([)]+)\))?\s*(\[(?<language>[^\]]+)\])?(?<tail>.*)?`;

export function pluginInfo() {
  return {
    namespace: "regex-filename-plugin",
    type: "metadata" as const,
    parameters: [
      {
        name: "trailing_tags",
        description:
          'If the filename ends with a pair of curly braces, return the contents inside them as a list of simple tags, without the "parsed:" namespace',
        required: false,
      },
      {
        name: "keep_all_captures",
        description:
          'Capture everything you find between a pair of parentheses and make it available under the "parsed:" namespace',
        required: false,
      },
      {
        name: "regex",
        description: "Regex to use for parsing (named capture groups; see legacy docs)",
        required: false,
      },
    ],
    declared_permissions: { net: [], read: false, write: false },
    name: "Filename Parsing",
    author: "Difegue (ported)",
    description:
      'Derive tags from the filename of the given archive, following the doujinshi naming standard "(Event) [Artist] TITLE (Series) [Language]" by default.',
    version: "1.2",
  };
}

function trim(s: string | undefined): string {
  return (s ?? "").trim();
}

function classifyItem(item: string, namespace: string): string {
  if (namespace && (COMMON_EXTRANEOUS_VALUES.has(item.toLowerCase()) || !isNaN(Number(item)))) {
    return PLUGIN_TAG_NS + item;
  }
  return `${namespace}${item}`;
}

function parseCapturedValueForNamespace(capture: string | undefined, namespace: string): string[] {
  if (!capture) return [];
  return capture
    .split(",")
    .map((s) => trim(s))
    .filter(Boolean)
    .map((s) => classifyItem(s, namespace));
}

function parseArtistValue(artist: string): string[] {
  const tags: string[] = [];
  const circleMatch = artist.match(/(.*) \((.*)\)/);
  let value = artist;
  if (circleMatch) {
    tags.push("group:" + trim(circleMatch[1]));
    value = trim(circleMatch[2]);
  }
  tags.push(...parseCapturedValueForNamespace(value, "artist:"));
  return tags;
}

interface ParseParams {
  checkTrailingTags: boolean;
  keepAllCaptures: boolean;
  regexString: string;
}

export function parseFilename(rawFilename: string, params: ParseParams): { tags: string; title: string } {
  const filename = rawFilename.replace(/_/g, " ");

  let match: RegExpMatchArray | null;
  try {
    match = filename.match(new RegExp(params.regexString));
  } catch {
    return { tags: "", title: "" };
  }
  if (!match || !match.groups) return { tags: "", title: "" };

  const captures = { ...match.groups };
  const title = trim(captures.title);
  let tail = trim(captures.tail);

  let trailingTags: string | undefined;
  let otherCaptures: string | undefined;

  if (tail) {
    if (params.checkTrailingTags) {
      const trailingMatch = tail.match(/(?<head>.*)(\{(?<ttags>[^}]*)\})$/);
      if (trailingMatch?.groups?.ttags) {
        trailingTags = trailingMatch.groups.ttags;
        tail = trim(trailingMatch.groups.head);
      }
    }
    if (tail && params.keepAllCaptures) {
      const items = [...tail.matchAll(/\(([^)]+)\)|\{([^}]+)\}|\[([^\]]+)\]/g)].map(
        (m) => m[1] ?? m[2] ?? m[3] ?? "",
      );
      otherCaptures = items.map((i) => trim(i)).filter(Boolean).join(",");
    }
  }

  let tags: string[] = [];
  for (const [rawName, rawValue] of Object.entries(captures)) {
    if (rawName === "title" || rawName === "tail" || rawValue === undefined) continue;
    const value = trim(rawValue);
    if (!value) continue;
    const namespace = rawName.replace(/\d+$/, "");

    if (namespace === "tag") {
      tags.push(...value.split(",").map((s) => trim(s)));
    } else if (namespace === "artist") {
      tags.push(...parseArtistValue(value));
    } else if (namespace === "event") {
      tags.push(`event:${value}`);
    } else {
      tags.push(...parseCapturedValueForNamespace(value, `${namespace}:`));
    }
  }

  if (otherCaptures) tags.push(...parseCapturedValueForNamespace(otherCaptures, PLUGIN_TAG_NS));
  if (trailingTags) tags.push(...parseCapturedValueForNamespace(trailingTags, ""));

  if (!params.keepAllCaptures) {
    tags = tags.filter((t) => !t.startsWith(PLUGIN_TAG_NS));
  }

  return { tags: [...new Set(tags)].sort().join(", "), title: title ?? "" };
}

interface ExecMetadataArgs {
  archive_id: string;
  file_path?: string;
  parameters?: { trailing_tags?: boolean; keep_all_captures?: boolean; regex?: string };
}

export function execMetadata(args: ExecMetadataArgs) {
  const filePath = args.file_path ?? "";
  const base = filePath.split(/[/\\]/).pop() ?? filePath;
  const filename = base.replace(/\.[^.]+$/, ""); // strip extension, matching Mojo::File->basename

  const { tags, title } = parseFilename(filename, {
    checkTrailingTags: args.parameters?.trailing_tags ?? false,
    keepAllCaptures: args.parameters?.keep_all_captures ?? false,
    regexString: args.parameters?.regex || DEFAULT_REGEX,
  });

  if (!tags && !title) return { tags: "" };
  return { tags, title: title || undefined };
}
