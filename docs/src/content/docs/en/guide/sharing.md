---
title: Sharing and importing
description: Share archives and tankoubons for others to read, download or import into their own LANrurugi; import someone else's share; embed a share in a web page.
---

Sharing hands part of your library to someone else: they can read it in a browser, download it, or
import it straight into their own LANrurugi instance, tags, summaries and tankoubon structure
included. Only a logged-in administrator can create shares; guest mode shows no sharing features.

## Creating a share

Right-click one or more archives or tankoubons in the library (mixed selections are fine), or open
the reader's menu, and choose **Share…**.

- **New share**: set the options below and generate a code.
- **Add to an existing share**: add the selection to a share that is still valid, optionally
  resetting its download limit and IP rules.

You get:

- a share code (for example `K7QM9-XRT2H`), its full text form (`lrrg:K7QM9-XRT2H?r=…`) and its QR code;
- when a **public address** is set, a browser link `…/s/<code>`, plus embed snippets if embedding is on.

The full text form carries this instance's current routes (addresses, relays, nodes), sealed so no
address shows in the text. Importing with it dials those routes directly, with no public service
involved; the bare code also works, but then the instance is found through public lookup services
or nodes.

The code is only shown when it is created and under "Show code". The instance stores a fingerprint
and an encrypted copy, so the code cannot be read back out of the database.

### Permissions

Any combination of three:

| Permission | What the recipient can do |
|---|---|
| Import | import the items into their own LANrurugi |
| Read | page through them in a browser, jumping by chapter where they have chapters |
| Download | download archive files in a browser; a tankoubon comes as one zip |

An import-only share shows just the code and QR on its page and card, with no browser reading.

### Limits

- **Download limit**: every import and every browser download counts once. Resuming or retrying the
  same import never counts again. When it is used up, only downloading stops: the share page and the
  embedded card still show their content and reading online still works, the Download action is
  simply gone, and new downloads and imports are refused (an import gets 409). An import already
  under way carries on.
- **IP rules**: only the listed addresses or ranges, or "only the first N distinct addresses". A share
  with IP rules needs the recipient to be reachable directly; a relayed connection cannot prove its
  address and is refused.
- **Expiry**: the share stops working at that time.
- **Short code**: 8 characters, easier to read out, but it needs an expiry within 7 days.

### Managing and revoking

**Settings → Sharing** lists every share. Expand a row to see its items, each claim's time and
address, the instances helping to deliver it, and to change any option.

**Revoke** takes effect at once: running imports stop within half a minute, and links and embedded
cards show "Revoked". Items deleted from the library show as "No longer available"; the rest of the
share is unaffected.

## Importing someone else's share

On the **Upload** page, click **Import share**, then:

- paste or type the code (case, spaces and hyphens don't matter; a whole `lrrg:` text or a share link
  works too);
- paste a screenshot of the QR code, or upload a QR image;
- or scan it with a camera.

QR codes are decoded entirely in your browser; images are never uploaded.

:::note[Over plain HTTP]
Browsers only allow camera access on HTTPS (or `localhost`) pages. When the instance is opened over
HTTP, the scan button becomes "Take photo", which opens the phone's camera to take a picture of the
QR code instead. The result is the same.
:::

A preview comes first: title, covers, count and total size, and which archives you already have
(those are not downloaded again). Pick a category and confirm; the import appears as one item in the
download queue, where it can be paused, resumed and expanded per archive.

- Every piece of data is verified before it is written; the finished files are byte-identical to the
  sharer's.
- An interrupted import (either side restarting, a dropped network) resumes from what was verified.
- Tankoubons are rebuilt in their original volume order, with titles, tags, summaries and chapters;
  your own tags on archives you already have are kept and merged, and an archive that already has
  chapters keeps yours. Bookmarks and stamps are the sharer's own and are not imported.

### Files and name collisions on import

Writing an import to disk is all Rust: verified pieces are written at their offsets into a `.part`
file, renamed to `.staged` when complete, and then moved into the library by the catalogue step —
no external unzip or copy command is involved.

The preview is a table — cover, title and size per archive — whose headers sort by title or size.
That ordering is for reading only; the import follows the sharer's manifest order.

What happens to a colliding name is chosen once in the import dialog (**When a local archive has
the same filename**): rename automatically (the default), skip that archive, or replace the local
archive. The preview says how many archives collide. The three behaviours:

- **the file already exists in the library directory**: it gets ` (2)`, ` (3)`… — not an error;
- **same name, different content** (a different archive): it is catalogued under a free name too, and
  the import row says "renamed to …";
- **identical content hash**: marked as already present and linked to the existing archive, using no
  extra space.

A local write failure — disk full, a read-only directory, a quota, missing permission — fails
that archive **immediately** instead of retrying against the same disk, and the row shows a
localized reason (the raw OS error stays in the tooltip). Exceeding `tempmaxsize` for the import's
temp space behaves the same way.

A `.patch.zip` archive needs one note: the sharer serves the **merged** bytes, so what lands locally
is the merged single file under the sharer's filename, with no patch sidecar. If a base archive with
that name already exists, the rules above rename it.

A tankoubon imports its members one by one under those rules, then recreates the tankoubon locally
under its original name and member order; an existing tankoubon with the same name and exactly the
same members is left alone and the import row says so.

### Helping deliver

Tick **Help deliver this share to others** when importing. If the sharer allows helpers, your
instance offers its verified copy to other importers once your import is done, taking load off the
sharer. Everything other importers receive is still checked against the sharer's fingerprints, so a
helper cannot alter the content.

Turn it off any time under **Settings → Sharing → Helping deliver**, which also shows each share's
state: active, paused (for example, downloads used up), sharer offline, or ended (revoked, expired,
or the sharer no longer accepts helpers). Ended rows can be removed from the list; imported archives
stay in your library.

## Embedding in a web page

With a public address set and **Allow embedding** on for the share, the share result offers two
snippets; use either:

```html
<iframe src="https://your-address/embed/s/CODE" width="360" height="420" style="border:0;max-width:100%" loading="lazy" referrerpolicy="no-referrer"></iframe>
```

```html
<script src="https://your-address/embed/share.js" defer></script>
<lrrg-share code="CODE" src="https://your-address"></lrrg-share>
```

Both show the same card: covers, title, count, and Read / Download / Import actions as the
permissions allow. It is fluid from 260 px wide.
A revoked or expired share shows only its state, with its actions greyed out; a used-up one still
shows its content, without a Download action and with a line saying the downloads are gone. An
IP-restricted share only says it is restricted.

With more than four items, a "+N" tile after the covers opens the full list, and hovering a cover
enlarges it. The card uses your instance's current theme; to pick another, add `?theme=…` to the
iframe address or a `theme="…"` attribute to `<lrrg-share>`: `g`, `ex`, `modern`, `modern_red`,
`modern_clear`, `light`, `dark`, or `system` to follow the visitor's light or dark setting. To change
only some colours, pass an object instead: `base` is the preset underneath, and `bg`, `fg`, `muted`,
`border`, `accent` (buttons and highlights), `tile` (cover placeholders) and the QR code's `qrFg`,
`qrBg` and `qrFinder` (finder squares) override it; every key is optional and only valid CSS colours
are used. Each preset has its own QR colours; keep enough contrast between QR foreground and
background when you set them. Set it as `el.theme = { base: "dark", accent:
"#f2aa4c" }` on the element, as JSON in its `theme` attribute, or URL-encoded as the iframe's
`?theme=`. The share dialog's "Embed in a web page" section builds the snippet for you, with a live
preview. Its text
follows the visitor's browser language (Chinese, English or Japanese); `?lang=zh` (`en`, `ja`) fixes
it.

## Connectivity

Shares travel directly between instances, and instances without a public address can still import
from each other. For the best speed, make the instance's **UDP port** (11204 by default, changeable
under **Settings → Sharing → Connectivity**) reachable from outside.

A system proxy needs no setup: imports and exports follow the proxy environment variables
(`HTTPS_PROXY`, `ALL_PROXY` and so on) or the macOS system proxy on their own, and a TUN-mode proxy
is transparent anyway. The proxy carries only relayed connections; direct connections between
instances bypass it.

When your reverse proxy runs in **another container**, a share's IP rules and the failed-attempt
limits see the proxy's address. Add its range under **Settings → Sharing → Connection → Trusted
proxies** (loopback only by default); requests from those addresses may then set the client address
with `X-Forwarded-For`. Private ranges are not trusted by default: anyone on the same network can
write that header.

To avoid public services, list your own or a friend's nodes under **Settings → Sharing → LANrurugi
nodes**; they then handle lookup and relaying. When this instance relays for others, you can cap
each connection's speed or admit only listed instances.

## Version compatibility

Importing and helping to deliver run on a **sharing engine version** of their own — an object, not a
single number:

| Field | Meaning |
|---|---|
| `protocol` | Wire-protocol epoch: the shape of every message, and the handshake itself |
| `transport` | Transport generation (currently `iroh 1.4.1`). iroh's QUIC/relay protocol has no version negotiation, so it moves together with `protocol` |

Both fields must match before two instances can import from or help deliver to each other. The check
happens in the first frame: the engine travels in its own part of the frame, encoded separately from
the message, so a peer whose messages this build cannot decode can still be told its engine is
unsupported — an incompatibility is never reported as a corrupt file or a silent hang-up.

A mismatch names both sides ("Invalid versions: the other side's sharing engine is v2 (iroh 1.4.0);
this instance is v3 (iroh 1.4.1)"); the fix is to upgrade the older side so both run the same
version. An import already under way is unaffected and resumes afterwards. When the **transport**
itself is incompatible the connection never completes, which surfaces as "the sharer is unreachable"
— that message also points at comparing both engines.

The share page, the embedded card, covers and reading online all use the ordinary HTTP API, where new
fields never conflict, so reading an older share never needs an upgrade. Your instance's engine is
shown under **Settings → Sharing → Status**.

## Upload speed

**Settings → Sharing → Upload limits** caps upload speed and the number of importers served at once,
separately for your own shares and for helping. As in BitTorrent clients, an alternative set of
limits can be switched in on a schedule (for example 01:00–08:00 daily) or by hand. An alternative
own-share speed of 0 pauses uploads: importers keep what they received and continue later; browser
reading is unaffected.
