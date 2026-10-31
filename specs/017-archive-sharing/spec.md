# Feature Specification: Archive & Tankoubon Sharing Across Instances

**Feature Branch**: `017-archive-sharing`

**Created**: 2026-10-24

**Status**: Draft

**Input**: User description: "分享功能（issue #119）：管理员把档案、单行本（单个或多个，可混合）分享出去，对方可在网页浏览，或导入到自己的 LANrurugi 实例。入口：主页右键菜单与漫画阅读页，可新建分享或添加到已有分享（添加时可重设下载次数和 IP 限制）；分享管理页可查看下载次数、改限制、撤销。限制：下载次数上限、下载 IP 限制。以简短分享码或二维码传递；二维码识别与生成都在前端完成，粘贴的图片在浏览器内解码不上传。上传页加「导入分享」按钮与弹窗，可粘贴/手动输入分享码或粘贴/上传二维码图片，预览后导入，单行本按原结构重建，元数据标签一并导入，复用下载队列与进度。分享内容可像推文卡片一样以 iframe 或 Web Component 嵌入其他网页。跨实例传输必须考虑无域名、无公网 IP 的实例：直连、打洞、中继兜底、类 BT 多实例分担与分块校验续传、局域网发现加速；分享码要解析到当前可用的连接方式而非写死 URL。只有登录用户（管理员）可分享，访客不可用。"

## Context

A LANrurugi library today has no way to hand part of itself to someone else. The only options are
sending archive files out of band, or opening the whole library to guests, and both lose the
tankoubon structure, the tags and the metadata the owner curated.

This feature lets an administrator pick archives and tankoubons, wrap them in a **share**, and pass
it on as a short **share code** or a QR code. Whoever receives it can either browse the share in a
web browser, or paste the code into their own LANrurugi instance and import the content with its
structure, tags and metadata intact.

Many instances run at home behind a router, often on a dynamic address with no domain name or no
public IP at all. A share that only works when the recipient can reach the sharer's address
directly would fail for exactly these users. So the code is never a hard-coded address: it is
resolved, at the moment of use, to whatever route currently works. In order of preference:

1. the same local network;
2. a direct public address;
3. a connection opened through both routers;
4. a relay as the last resort.

When several instances already hold the content, they can help deliver it, with every piece
verified against what the sharer published.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Create a share and let someone browse and download it (Priority: P1)

An administrator right-clicks an archive or a tankoubon in the library, or uses the share action
on the reader page, and creates a share. They choose what the code permits — **import** into
another LANrurugi instance, **read** online, **download** files, any combination — and optionally set
a download limit and an IP restriction. They get back a short code, a QR code and a link. A recipient with only a web browser
opens the link or enters the code and sees the share: covers, titles, tags and page counts. They
can read it online and download the archive files.

**Why this priority**: it is the smallest slice that delivers the core value: a curated selection
leaves the library under the owner's control. Every other story builds on the share it creates.

**Independent Test**: on an instance reachable by address, create a share of one tankoubon and one
loose archive with a download limit of 2. Open the share link from another device. Delivered value:
the recipient sees both items, can read pages online and download files. After two downloads, a
third is refused with a clear message.

**Acceptance Scenarios**:

1. **Given** an administrator is logged in, **When** they right-click an archive or tankoubon in
   the library, or open the share action on the reader page, **Then** they can create a new share
   containing it, or add it to an existing share.
2. **Given** several archives and/or tankoubons are selected in the library, **When** the
   administrator chooses share, **Then** all selected items go into one share.
3. **Given** a share is created, **When** creation completes, **Then** the administrator sees its
   share code, a QR code and a link, each of which can be copied, and the QR code can also be saved
   as an image.
4. **Given** items are being added to an existing share, **When** the administrator confirms,
   **Then** they may keep or replace that share's permissions, download limit and IP restriction
   in the same step.
5. **Given** a recipient opens a valid share, **When** the page loads, **Then** they see every item
   with cover, title, tags and page count, and tankoubons show their member archives in order.
6. **Given** a share has a download limit of N, **When** the (N+1)th distinct recipient tries to
   download, **Then** the download is refused, and browsing still works unless the share has been
   revoked or has expired.
7. **Given** a share restricts access to certain IP addresses or ranges, **When** a request comes
   from outside them, **Then** browsing, reading and downloading are all refused, and the response
   does not reveal what the share contains.
8. **Given** a visitor is not logged in as an administrator (including guest-mode visitors and
   guest tokens), **When** they use the library or reader, **Then** no share action is offered, and
   any attempt to create or manage shares is refused.
9. **Given** guest mode is off, or a shared archive is in a category not visible to guests,
   **When** a recipient who is not logged in opens the share with its code, **Then** they can
   still browse, read and download everything in the share, because the code is what grants
   access.
10. **Given** a recipient opened a share, **When** they try to reach any page, cover, file or
    search result outside it, with or without the code, **Then** the request is treated as if they
    had no code at all: the normal login or guest rules apply.
11. **Given** a share is created, **When** the administrator picks its permissions, **Then** they
    can select any non-empty combination of import, read and download. A share with none selected
    cannot be saved.
12. **Given** a share's code does not permit read, **When** a recipient opens it, **Then** they see
    the item list (covers, titles, tags, page counts) but cannot open any page in the reader.
    Likewise, without download no file can be downloaded in the browser, and without import no
    other instance can import it. The share page only shows the actions the code permits.

---

### User Story 2 - Manage existing shares (Priority: P1)

The administrator opens the **Sharing** section of the settings page, which lists every share with:

- its items;
- how many downloads it has used out of its limit;
- which addresses have downloaded it, and when;
- its status (active, exhausted, expired, revoked).

From there they can change the limits, remove items, show the code or QR again, or revoke the
share.

**Why this priority**: a share with limits is not trustworthy unless the owner can see whether it
is being used and stop it. This ships together with Story 1.

**Independent Test**: create a share, download it once from another device, then open the
Sharing section of the settings page. Delivered value: it shows 1 download with that device's address. After
revoking, the code no longer opens anything.

**Acceptance Scenarios**:

1. **Given** shares exist, **When** the administrator opens the settings page's Sharing section, **Then** each share
   shows its items, used/allowed downloads, the addresses that downloaded and when, its creation
   time and its status.
2. **Given** a share, **When** the administrator raises, lowers or removes its download limit or
   changes its IP restriction, **Then** the new values apply to every later request.
3. **Given** a share, **When** it is revoked, **Then** its code, link, QR code and embeds stop
   working immediately, including downloads already in progress. A revoked share cannot be
   reactivated; a new share must be made instead.
4. **Given** an item in a share is deleted from the library, **When** the share is viewed,
   **Then** the item is shown as no longer available and the rest of the share keeps working.

---

### User Story 3 - Download a whole tankoubon (Priority: P2)

Today only single archives can be downloaded: the library context menu hides "Download" for
tankoubons and the reader only offers the current archive. An administrator now picks "Download" on
a tankoubon, in the library context menu or on the reader page while reading inside a tankoubon,
and gets one file containing all its member archives in tankoubon order. A share recipient whose
code permits download gets the same for a shared tankoubon.

**Why this priority**: a share's download permission is meaningless for tankoubons without it,
and it is useful on its own, without any share. It does not depend on the other stories, so it
can be built and tested first.

**Independent Test**: on a tankoubon of three archives, choose Download from the library context
menu. Delivered value: one file arrives. Opening it yields the three original archive files,
byte-identical to the ones in the library, named so that they sort in tankoubon order.

**Acceptance Scenarios**:

1. **Given** an administrator, **When** they open the context menu of a tankoubon in the library,
   **Then** "Download" is offered, as it is for an archive.
2. **Given** an administrator reading an archive inside a tankoubon, **When** they open the reader's
   download action, **Then** they can download either the current archive or the whole tankoubon.
3. **Given** a tankoubon is downloaded, **When** the file is opened, **Then** it contains every
   member archive as its original file, unchanged, with names prefixed by their position, for
   example `01 - <title>`, so they list in tankoubon order.
4. **Given** a large tankoubon, **When** the download starts, **Then** it starts right away and
   streams. It does not wait for a packaged copy to be prepared first, and it needs no temporary
   copy of the whole tankoubon on the server.
5. **Given** a member archive's file is missing from disk, **When** the tankoubon is downloaded,
   **Then** the other members are still delivered, and the downloaded file says which member is
   missing.
6. **Given** a shared tankoubon in a share that permits download, **When** a recipient downloads
   it, **Then** they get the same file, and it counts within their one claim against the share's
   download limit.
7. **Given** a visitor who is not an administrator and holds no share code permitting it, **When**
   they try to download a tankoubon, **Then** the request is refused.

---

### User Story 4 - Import a share into another LANrurugi instance (Priority: P2)

An administrator of another instance opens the upload page and chooses "Import share". In the
dialog they can:

- type or paste the share code;
- paste a screenshot that contains the QR code;
- choose an image file that contains the QR code;
- on a phone or any device with a camera, scan the QR code live with the camera.

The instance resolves the code and shows a preview of the items. After confirmation it imports
them: archives arrive with their tags and metadata, and tankoubons are rebuilt with the same
members in the same order. Progress shows in the download queue like any other download.

**Why this priority**: importing with structure and metadata intact is what sets this apart from
sending files by hand. It depends on the share from Story 1 but not on any connectivity beyond a
directly reachable sharer.

**Independent Test**: with two instances that can reach each other, share one tankoubon of three
archives from instance A. On instance B, paste a screenshot of the QR code into the import dialog.
Delivered value: B shows a preview, then imports three archives with their tags and one tankoubon
containing them in the original order, with progress visible in B's download queue.

**Acceptance Scenarios**:

1. **Given** the import dialog is open, **When** the administrator pastes or chooses an image
   containing a share QR code, **Then** the code is read in the browser, the image is never
   uploaded to any server, and the resulting code fills the input.
2. **Given** an image contains no readable share code, **When** it is supplied, **Then** the
   dialog says so and nothing is imported.
3. **Given** the dialog is opened on a device with a camera, over a connection where the browser
   allows camera access, **When** the administrator chooses to scan, **Then** a live camera view
   reads the QR code as soon as it is in frame. Recognition happens in the browser, no frame
   leaves the device, and the camera is released as soon as a code is read or the scan is closed.
4. **Given** the browser does not allow live camera access, for example because the instance is
   opened over plain HTTP by local address, or the user denied permission, **When** the dialog
   opens, **Then** it does not offer a scan that will fail. It instead offers taking a photo with
   the device's camera app, which is then decoded like any chosen image, and says briefly why live
   scanning is unavailable.
5. **Given** a valid code, **When** it is submitted, **Then** a preview lists the items: titles,
   covers, archive count, total size, and which items already exist in this library.
6. **Given** the preview is confirmed, **When** the import runs, **Then** it appears in the
   download queue as **one compact entry**, however many archives it has. The entry shows archives
   done/total, bytes done/total, speed and time remaining. It expands to a scrollable per-archive
   list, where any archive can be retried. The whole import can be paused, resumed after
   interruption, and stopped.
7. **Given** an archive in the share already exists in the receiving library with identical
   content, **When** importing, **Then** it is not downloaded again, and it is still placed in any
   imported tankoubon that contains it.
8. **Given** the import finishes, **When** the administrator looks at the result, **Then**
   archives carry the shared titles, tags and metadata. Each tankoubon exists with the same name,
   members and order as on the sharing instance.
9. **Given** the share's download limit is exhausted, the share was revoked, or the importing
   instance's address is not allowed, **When** the import is attempted, **Then** it is refused
   before anything is queued, with a message naming the reason.
10. **Given** one import of a share, **When** it is paused, resumed, retried or interrupted and
   continued, **Then** it still counts as one download against the share's limit.
11. **Given** the sharer's schedule pauses own-share uploads, **When** an import is running,
   **Then** the import shows "sharer has paused uploads until <time>", keeps its progress, and
   continues on its own at that time.
12. **Given** the importing instance's network routes traffic through a proxy (environment
   variables, a TUN or transparent proxy, or a router-level proxy), including one that drops UDP,
   **When** a share is imported, **Then** it completes with no LANrurugi setting changed, over the
   relay route if direct UDP does not work.

---

### User Story 5 - Share codes work without a domain or a public IP (Priority: P2)

The sharing instance sits behind a home router with a changing address, or behind carrier-grade
NAT with no public IP at all. Its share code still works.

- An importing instance on the same local network finds it and transfers at local-network speed.
- An importing instance elsewhere connects through both routers when that is possible, and through
  a relay when it is not.
- A recipient with only a browser opens the share on the sharer's own instance, which needs the
  sharer to be reachable directly (a public address or domain, or the same local network).
  Browser access never goes through any third-party server.

The code itself never changes when the sharer's address changes.

**Why this priority**: the user named this explicitly: home deployments without a domain or public
IP are the common case for this project's users. Without it, sharing only works for the minority
with a public address.

**Independent Test**: put the sharing instance behind NAT with no port forwarding.

- Import its share from an instance on a different network: delivered value is a successful
  import.
- Repeat from an instance on the same LAN: delivered value is that the transfer uses the local
  route.
- Change the sharer's public address mid-way: delivered value is that the same code still
  resolves.

**Acceptance Scenarios**:

1. **Given** the sharer has a public address or domain, **When** a share is resolved, **Then** the
   direct route is offered.
2. **Given** sharer and recipient are on the same local network, **When** a share is resolved,
   **Then** the local route is preferred over any route through the internet.
3. **Given** neither side is directly reachable, **When** a share is resolved, **Then** the systems
   first try to connect through both routers. Only if that fails do they fall back to a relay, and
   the import still completes.
4. **Given** the sharer's address changes, **When** an existing code is used, **Then** it resolves
   to the new address without a new code being issued.
5. **Given** a transfer passes through a relay, rendezvous point or other instance, **When** it
   does, **Then** none of them can read the shared content or reconstruct the share code from what
   passes through them.
6. **Given** the sharing instance is offline, **When** its code is used, **Then** the recipient is
   told the sharer is currently unreachable (as distinct from an invalid or revoked code), and an
   in-progress import resumes once it is back.
7. **Given** a transfer uses a relay, **When** the share has an IP restriction, **Then** it is
   evaluated against the recipient's real address once a direct connection is established. If none
   can be established, the share is refused with "address could not be verified", and it is never
   evaluated against the relay's address.
8. **Given** an administrator who does not want their instance to use outside rendezvous or relay
   services, **When** they turn that off, **Then** sharing still works over direct and local routes
   and any LANrurugi nodes they configured (FR-048). With no node configured, the share page says
   the share needs a direct route.

---

### User Story 6 - Embed a share in another web page (Priority: P3)

The administrator turns on embedding for a share and copies an embed snippet. Pasted into a forum
post, blog or other page, it renders a compact card like an embedded social-media post, showing:

- cover thumbnails;
- the share's title;
- the number of items;
- a link to open the share.

**Why this priority**: it widens how a share can be presented, but nothing else depends on it.

**Independent Test**: enable embedding on a share, paste the snippet into a static HTML page on
another site and open it. Delivered value: the card renders, and clicking it opens the share. After
revoking the share, the card shows that the share is no longer available.

**Acceptance Scenarios**:

1. **Given** a share, **When** embedding has not been turned on for it, **Then** no embed renders
   and no content is exposed through the embed route.
2. **Given** embedding is on, **When** the administrator asks for the snippet, **Then** they can
   copy both a frame-based snippet and a custom-element snippet. Either one renders the same card.
3. **Given** shares with different permissions, **When** each is embedded, **Then** each card
   offers only the actions its share permits:
   - a read-only share's card leads into the reader;
   - a download share's card offers the file;
   - an import-only share's card features the code and QR code for importing.
4. **Given** an embedded card, **When** the share is revoked, expired or exhausted, **Then** the
   card shows that state instead of the content.
5. **Given** a share with an IP restriction, **When** it is embedded, **Then** the card shows only
   that the share exists and is restricted, never its covers or titles.
6. **Given** the card, **When** it renders, **Then** it adapts to light and dark host pages and to
   narrow widths without overflowing.

---

### User Story 7 - Instances that already hold a share help deliver it (Priority: P3)

The sharer has allowed helping on a share. Several instances have already imported it and chose
to help deliver it. A new
importer then downloads pieces from them as well as from the sharer, much like peer-to-peer file
distribution. Every piece is checked against the sharer's published fingerprints, so a corrupted or
tampered piece is discarded and fetched again elsewhere.

**Why this priority**: it speeds up popular shares and reduces load on a home connection, but a
share is fully usable without it.

**Independent Test**: share one large archive from A; import it fully on B with "help deliver"
enabled; then import on C while throttling A's upload. Delivered value: C completes noticeably
faster than A alone allows, C's file is byte-identical to A's, and a deliberately corrupted piece
served by B is rejected and fetched again.

**Acceptance Scenarios**:

1. **Given** an instance has imported a share, **When** it has not opted in to helping deliver it,
   **Then** it never serves that content to anyone.
2. **Given** a share whose sharer has not allowed helping (the default), **When** it is imported,
   **Then** no instance can register as a helper for it, and every import involves only the sharer
   and the importer.
3. **Given** a share that allows helping, **When** an importer previews it, or an administrator is
   about to opt in to helping deliver it, **Then** the dialog states that the instances involved
   in multi-source delivery can see each other's IP addresses.
4. **Given** helpers exist, **When** a new importer downloads, **Then** it may fetch pieces from
   helpers and the sharer at the same time, and verifies each piece before using it.
5. **Given** a piece fails verification, **When** it is received, **Then** it is discarded and
   fetched again from another source, and the source that sent it is no longer used for that
   transfer.
6. **Given** the sharer has revoked the share, lowered its limit below what is used, or the
   importer is not allowed by the IP restriction, **When** the importer asks helpers for pieces,
   **Then** helpers refuse. Helpers only serve importers the sharer has authorised.
7. **Given** a helper instance's administrator turns helping off or deletes the archive, **When**
   that happens, **Then** it stops serving immediately and importers continue from other sources.
8. **Given** a share a helper is delivering, **When** the sharer revokes it, it expires, helping is
   turned off, its downloads run out, or some of its archives are removed from it, **Then** the
   helper's administrator sees that share's new state, the reason, and when it happened.
   - Ended shares stay listed until the administrator removes them from the list.
   - A share whose downloads ran out shows as paused, and resumes on its own if the sharer raises
     the limit.
9. **Given** the sharer has not been reachable for a while, **When** the helper's administrator
   looks at the list, **Then** the share shows as "sharer offline" with the time it was last
   reached. This is distinct from an ended share, and clears on its own when the sharer is back.
10. **Given** upload limits are set (FR-041), **When** importers fetch from this instance, as the
    sharer or as a helper, **Then** uploads never exceed those limits, and the instance's other
    traffic is not starved.

---

### Edge Cases

- **Shared archive changes**: an archive's file is replaced or re-tagged after it was shared. A new
  import receives its current state. An import already in progress is not silently mixed between
  the old and new file: it restarts that archive.
- **Tankoubon membership changes**: a tankoubon's members change after sharing. The share shows the
  current members, and an import rebuilds the membership as it is at preview time.
- **Same item twice**: the same archive is added to a share twice, or is shared both on its own
  and inside a shared tankoubon. It is listed once per place it appears but transferred once.
- **Very large shares**: a share with hundreds of archives. The preview loads progressively, and
  the import enters the queue as one entry without blocking the dialog. On the sharing side, the
  first importer starts receiving archives that are already prepared while the rest are still
  being prepared.
- **Mistyped codes**: a manually typed code is wrong by one character. Look-alike characters
  (0/O, 1/I/l) are tolerated. Any other typo is reported as an invalid code after the lookup fails.
- **Proxied importer**: an importer sits behind a proxy whose exit address is shared by many users.
  IP restrictions and "first N addresses" see that shared exit address (FR-051a). An allow-list
  that names the importer's home address refuses it. The refusal is the same "not found" answer as
  any other (FR-014), so the importer's dialog adds a general hint that a proxy's exit address is
  what the sharer sees.
- **Domain changes**: the sharer's domain changes, or its dynamic IP changes, after codes were
  handed out. Codes and `lrrg:` forms keep working (FR-010a). Browser links on the old domain stop
  working; the recipient can still import with the code, or be given a new link.
- **Code guessing**: someone tries many codes. The code space and attempt limits make finding a
  valid code by guessing impractical, and a flood of attempts does not degrade the instance.
- **Two importers at once**: two importers start at the same moment when one download remains.
  Exactly one succeeds.
- **Expiry mid-import**: a share expires or is revoked while an import is running. The import stops
  and is marked failed with that reason. Already-completed archives stay in the receiving library.
- **Importing a share you made**: an instance imports its own share. It recognises the content as
  already present and imports nothing.
- **Changing addresses**: the recipient's address changes mid-import, for example a mobile network.
  It is still the same download for counting purposes. For IP restrictions, the new address must
  also be allowed.
- **Fake QR content**: the pasted image contains a QR code that is not a share code, such as a URL
  to an unrelated site. It is rejected, and the dialog never navigates to or fetches that content.
- **Camera unavailable or denied**: on a phone opening the instance over plain HTTP by local
  address, live camera access is not available. The dialog offers taking a photo instead. If the
  user denies camera permission, the dialog falls back the same way without repeated prompts.
- **Permissions removed mid-use**: a permission is removed while a recipient is using it, such as
  reading or importing. The next request that needs it is refused. An import in progress stops and
  is marked failed with that reason, and already-completed archives stay in the receiving library.
- **Shared item also guest-visible**: a shared item is also visible to guests. Opening it through
  the share and through guest browsing are independent: revoking the share does not affect guest
  access, and guest settings do not affect the share.
- **Duplicate titles on import**: the receiving library already has a tankoubon with the same name
  but different members. The import creates a separate tankoubon rather than merging into it.

## Requirements *(mandatory)*

### Functional Requirements

**Creating and managing shares**

- **FR-001**: Administrators MUST be able to create a share containing one or more archives and/or
  tankoubons, from the library's context menu (single item or current multi-selection) and from the
  reader page (the current archive, or the tankoubon it is being read in).
- **FR-002**: The same entry points MUST offer adding the item(s) to an existing active share. When
  adding, the administrator MUST be able to keep or replace that share's permissions, download
  limit and IP restriction.
- **FR-003**: Each share MUST have a set of **permissions**, chosen from import (into another
  LANrurugi instance), read (online, in the browser) and download (archive files, in the browser).
  Any non-empty combination is allowed. A valid code always allows seeing the item list (covers,
  titles, tags, page counts); everything beyond that requires the matching permission, enforced on
  every request. Permissions MAY be changed later, and a change applies to every later request.
  Each share MUST also have an optional download limit (default: unlimited), an optional IP
  restriction (default: none), an optional expiry time (default: none), and an optional title
  (default: derived from its first item).
- **FR-004**: The IP restriction MUST support:
  - (a) an allow-list of individual addresses and address ranges;
  - (b) "only the first N distinct addresses", which locks the share to whoever uses it first.
  
  The two may be combined.
- **FR-005**: The settings page MUST have a **Sharing** section, which is where shares are managed.
  It MUST list every share with its items, title, status (active, exhausted, expired, revoked),
  used and allowed downloads, the address and time of each download, and its creation time.
- **FR-006**: From the Sharing section, administrators MUST be able to:
  - change permissions and any limit;
  - remove items;
  - re-display the code, link and QR code;
  - turn embedding on or off;
  - revoke the share.
  
  Revocation MUST be immediate and permanent.
- **FR-006a**: The same Sharing section MUST also hold this instance's sharing-wide settings:
  - the connectivity options of FR-027 (rendezvous and relay choice, LANrurugi nodes, or public services off);
  - the helper opt-ins of FR-033, as a list of shares this instance helps deliver, each of which
    can be turned off;
  - the defaults that new shares start with (permissions, download limit, IP restriction, expiry,
    embedding).
  
  Like the share list, it is visible only to administrators.
- **FR-007**: Only authenticated administrators MAY create, view in management, modify or revoke
  shares. Guest-mode visitors and guest tokens MUST NOT see share actions, and MUST be refused by
  the server if they try.

**Codes and QR codes**

- **FR-008**: Each share MUST get a short share code that a person can read aloud and type:
  - **10 characters** by default, shown as two groups of five, for example `K7QM9-XRT2H`;
  - case-insensitive, with the usual look-alike characters (0/O, 1/I/L) treated as the same;
  - unpredictable enough that guessing a valid code is impractical.
  
  An administrator MAY instead choose a **short code** of 8 characters (`K7QM-9XRT`). This is
  allowed only for a share with an expiry no more than 7 days away, because the shorter code is
  easier to guess and the expiry bounds how long it can be guessed at.
- **FR-008a**: Like a `magnet:` link, a share MUST have a canonical text form with a fixed prefix,
  `lrrg:<code>`, for example `lrrg:K7QM9-XRT2H`. It may carry optional hints, for example
  `?via=<the sharer's node identity and direct address>` when the share can only be reached directly. The text form
  is what the share dialog offers for pasting into chats and posts. The import dialog MUST accept
  the text form, a share link, and a bare code.
- **FR-009**: The system MUST present each share as text form and QR code. When the sharer has a
  public address, it MUST also offer a link to the share page on the sharer's own instance, and
  the QR code MUST encode that link, so that a phone camera opens the page directly. Otherwise the
  QR code encodes the text form (FR-010a). The QR code MUST be generated in the browser, and MUST
  be savable as an image.
- **FR-010**: The share code MUST NOT embed a fixed network address. It MUST resolve, at the time
  of use, to the routes through which the sharing instance is currently reachable.
- **FR-010a**: Changing the sharing instance's domain, public address or IP MUST NOT invalidate any
  code or `lrrg:` text form already handed out: they resolve to the sharer's current address at
  the time of use.
  
  Browser links necessarily name the sharer's own address (FR-047), so a link stops working when
  that address does. The share dialog MUST say so next to the link. When no public address is
  configured, the QR code MUST encode the `lrrg:` text form for importing, instead of a link.
- **FR-047**: Connection information and transfer information MUST come from the sharing instance
  itself, over the end-to-end encrypted connection to it. Connection information means addresses,
  the public web address and the helper list; transfer information means the manifest, metadata,
  piece fingerprints and authorisations. They MUST never be stored on, served by or relayed in
  readable form through any other server. That includes servers run by this project, and helper
  instances: helpers supply piece data only, verified against fingerprints from the sharer.
  
  The only thing an outside service may hold is the minimum needed to knock on the sharer's door
  when it cannot be reached directly: an encrypted record that only code holders can read,
  containing the sharer's node identity and the relays it can be reached through. With public
  services off, it goes only to the LANrurugi nodes the administrator configured (FR-048), and with
  none configured it is not published at all.
- **FR-048**: Instances with a public address MAY offer themselves as rendezvous and relay nodes
  for other instances, for example friends' instances. Each instance MUST be able to list such
  nodes to use instead of, or alongside, public services. This lets a group of instances share
  without any third-party server as long as one of them is reachable.
- **FR-049**: When more than one rendezvous or relay node is configured, the instance MUST use them
  redundantly, so that any single one failing does not stop sharing. The sharing section of the
  settings MUST show which outside services are currently in use and whether each is reachable.
- **FR-011**: Failed code lookups MUST be rate-limited per requester, so that repeated guessing
  neither succeeds in practice nor degrades the instance.

**Recipient web access**

- **FR-012**: Anyone holding a valid code or link MUST be able to open the share in a browser,
  without an account, and see its items: cover, title, tags, page count, and member order for
  tankoubons.
- **FR-013**: Web recipients MUST be able to read shared archives online when the share permits
  read, and download individual archive files when it permits download. They MUST NOT be able to
  reach any part of the library outside the share.
- **FR-013a**: Recipient access MUST carry the share code on every request. The code is the
  credential: it MUST grant access to the share's items, and only those, whether or not guest mode
  is on and whatever categories the items are in. A request without a valid code, or for anything
  outside the share named by its code, MUST get exactly the access a not-logged-in visitor gets
  today. Opening a share MUST NOT give the recipient a library session or widen any guest scope.
- **FR-013b**: The share page and everything it loads MUST NOT pass the share code on to third
  parties, for example through the referring address when a recipient follows an external link.
- **FR-014**: Requests outside a share's IP restriction MUST be refused for browsing, reading and
  downloading alike, without revealing the share's contents. Requests for a revoked or expired
  share MUST be refused in the same way.
- **FR-015**: Downloads MUST count against the share's limit per **recipient claim**, not per file
  or per request:
  - an import of the whole share is one download, as is a browser recipient's download session;
  - pausing, resuming, retrying and fetching several files within the same claim count once;
  - reading online does not count;
  - when the limit is reached, new claims MUST be refused while existing ones may complete;
  - two simultaneous claims for the last remaining download MUST result in exactly one success.

**Import into another instance**

- **FR-016**: The upload page MUST offer an "Import share" action opening a dialog that accepts:
  - a typed or pasted share code;
  - a pasted image containing a share QR code;
  - a chosen image file containing a share QR code;
  - a live camera scan, where the browser allows camera access.
  
  Where live camera access is unavailable (no camera, permission denied, or an insecure
  connection), the dialog MUST offer taking a photo with the device's camera app instead, and say
  why live scanning is unavailable.
- **FR-017**: QR codes in supplied images and camera frames MUST be decoded in the browser. Images
  and frames MUST NOT be sent to any server, and content that is not a share code MUST be rejected
  without being followed. The camera MUST be released as soon as a code is read or scanning is
  closed.
- **FR-018**: Before importing, the dialog MUST show a preview: item titles, covers, archive count,
  total size, and which items already exist in the receiving library.
- **FR-019**: Confirmed imports MUST go through the existing download queue as **one entry per
  import**, not one per archive:
  - collapsed, it occupies no more space than a single download's entry, and shows archives
    done/total, bytes done/total, speed and remaining time;
  - expanded, it lists every archive with its state and progress. The list scrolls within a
    bounded height, and failed archives can be retried individually;
  - the entry supports pause, resume after interruption or restart, and stop for the whole
    import.
- **FR-019a**: Imports MUST NOT block other work:
  - an import transfers a bounded number of archives at once (default 3, configurable in the
    sharing settings);
  - imports never hold slots that other downloads in the queue wait on;
  - a long import never delays reading, browsing or other requests on either instance.
  
  On the sharing side, preparing archives for transfer (fingerprinting) MUST run in the background
  from the moment items are shared, at a bounded share of CPU. A first import MUST be able to start
  transferring prepared archives while the rest are still being prepared.
- **FR-020**: Imported archives MUST carry the sharing instance's title, tags and metadata. Each
  shared tankoubon MUST be recreated with the same name, members and order. An archive already
  present with identical content MUST be reused rather than downloaded again.
- **FR-021**: An import MUST be refused before anything is queued if the share does not permit
  import, is revoked, expired or exhausted, or the importer is not allowed by its IP restriction,
  with the reason shown.
- **FR-022**: Every instance-to-instance transfer MUST be chunked and verified chunk by chunk,
  whatever its source (sharer or helper):
  - each archive is transferred as independently requestable pieces;
  - each piece is checked against the sharer's published fingerprint as it arrives, and data that
    fails the check is never written. The failed piece is fetched again;
  - which pieces have been verified is recorded, so an interrupted transfer resumes by fetching
    only the missing pieces;
  - once all pieces are present, the whole archive MUST match the published fingerprint before it
    is added to the library. A mismatch MUST fail that archive, never silently add it.

**Connectivity**

- **FR-023**: Transfers MUST prefer, in order: the same local network; a direct address; a
  connection opened through both sides' routers; and, only when none of these works, a relay.
- **FR-024**: Instances on the same local network MUST be able to discover each other for a share
  without any manual configuration.
- **FR-025**: An instance with no public address and no domain MUST be able to share with
  importing instances elsewhere, whenever any of the routes in FR-023 exists. Browser-only
  recipients reach a share only on the sharer's own instance, over a direct address or the local
  network (FR-047). For anyone else, the share page offers the code for import.
- **FR-026**: Relays, rendezvous services and helper instances MUST NOT be able to read shared
  content or derive the share code from traffic passing through them.
- **FR-027**: The rendezvous and relay services an instance uses MUST be configurable. By default
  the instance uses public services, so that sharing works without setup even when both sides are
  behind NAT. Administrators MUST be able to turn public services off entirely with one switch,
  which restricts sharing to direct and local routes and the nodes of FR-048.
- **FR-028**: Download limits and IP restrictions MUST be evaluated against the recipient's own
  address, never a relay's. On a route where the recipient's address cannot be observed (a relayed
  connection that cannot be upgraded to a direct one), a share with an IP restriction MUST be
  refused with a reason saying the address could not be verified. Shares without an IP restriction
  work on every route.
- **FR-029**: When the sharing instance cannot be reached at all, a recipient MUST be told the
  sharer is currently unreachable (as distinct from an invalid, revoked or expired code). An
  interrupted import MUST resume from where it stopped once the sharer is reachable again.

**Embedding**

- **FR-030**: Embedding MUST be off by default per share, and is available only when the sharer has
  a public address, since the card is served by the sharer's own instance (FR-047). When it is on, administrators MUST be
  able to copy a frame-based snippet and a custom-element snippet, and both MUST render the same
  card: covers, title, item count, and a link to open the share.
- **FR-030a**: The card MUST offer only the actions the share permits:
  - read: open the reader for an item;
  - download: download a file;
  - import: show the share code and QR code with a way to copy them, for importing into another
    LANrurugi instance.
  
  The card's layout MUST differ by permission set, so that, for example, an import-only card
  presents the code as its main element, not a read button.
- **FR-031**: An embedded card MUST show the share's current state (revoked, expired, exhausted)
  instead of its content. For an IP-restricted share, it MUST show only that a restricted share
  exists, never covers or titles.
- **FR-032**: The card MUST adapt to light and dark host pages and to narrow widths.

**Peer-assisted delivery**

- **FR-033**: An instance that has imported a share MUST NOT serve its content to others unless
  both of these hold:
  - the sharer allowed helping on that share (a per-share setting, off by default);
  - the instance's administrator opted in to helping deliver it.
  
  Opting out, or the sharer turning helping off, MUST take effect immediately. Without helping,
  only the sharer's and the importer's addresses are involved in a transfer.
- **FR-033a**: Wherever helping is offered — the sharer's toggle, the importer's preview, and the
  helper opt-in — the interface MUST state that multi-source delivery lets the participating
  instances see each other's IP addresses.
- **FR-034**: Importers MAY fetch pieces from the sharer and helpers at the same time. Every piece
  MUST be verified against the sharer's published fingerprints. A failed piece MUST be discarded
  and fetched again from another source, and the source that sent it MUST stop being used for
  that transfer.
- **FR-035**: Helpers MUST serve only importers that hold an authorisation from the sharer that is
  currently valid, and only for shares that permit import. The sharer's revocation, permissions,
  limits and IP restriction therefore bind helpers too.
- **FR-035a**: The sharer MUST tell each helper the state of every share it helps with: active,
  paused (downloads ran out), or ended (revoked, expired, or helping turned off), with the time of
  the change. It MUST also say which archives were removed from the share. The helper MUST:
  - stop serving ended shares and removed archives;
  - pause serving paused shares, and resume when the sharer reports them active again;
  - in the sharing section of its settings, show per share its state, the reason and time, the
    last time the sharer was reached ("sharer offline" after 15 minutes without contact), and the
    total data uploaded while helping;
  - keep ended shares in that list until the administrator removes them. Removing an entry never
    touches the imported archives.
- **FR-035b**: Helping MUST obey the upload limits of FR-041.

**Upload limits and schedule**

- **FR-041**: Administrators MUST be able to cap the upload traffic sharing causes, in the sharing
  section of the settings. There are two independent groups of caps:
  - **own shares**: the total upload rate for everything this instance serves for its own shares
    (imports by other instances, and file downloads by browser recipients), and the maximum number
    of importers served at once;
  - **helping**: the total upload rate for helping deliver other instances' shares, and the
    maximum number of importers served at once.
  
  Each value can be unlimited. Defaults: unlimited rate, 8 importers for own shares, 4 for
  helping.
- **FR-042**: Like a BitTorrent client's alternative speed limits, there MUST be a second,
  **alternative** set of the same caps, and a schedule for when it applies:
  - one or more time windows, for example 01:00–08:00;
  - optional days of the week;
  - the instance's own time zone.
  
  A rate of 0 in the alternative set pauses that kind of upload for the window. A manual switch
  MUST force the alternative set on until switched off, independent of the schedule. The settings
  MUST show which set is in effect now and when the next scheduled change happens. Schedule
  changes MUST take effect within one minute of a window's boundary.
- **FR-043**: Online reading and browsing by recipients MUST NOT be paused by these caps. A
  paused own-share upload delays only imports and file downloads.
- **FR-044**: When the sharer pauses own-share uploads, an importer MUST show that the sharer has
  paused uploads until a given time, keep its verified progress, and continue on its own when the
  pause ends. Helpers keep serving within their own caps meanwhile, provided the sharer is still
  issuing authorisations.

**Traffic profile**

- **FR-045**: Sharing traffic MUST NOT carry a protocol signature, always and without a setting.
  This covers code resolution, publishing, hole punching, transfers and relaying:
  - nothing an ordinary traffic classifier would recognise as BitTorrent or another P2P
    file-sharing protocol;
  - no plaintext identifier of LANrurugi or of the transport library in any packet;
  - every connection is encrypted and presents as a common encrypted protocol.
  
  This MUST NOT depend on the instance having a domain, a certificate or HTTPS of its own.
- **FR-046**: An instance on a network that blocks UDP, or allows only outbound encrypted web
  traffic, MUST still be able to share and import through the relay route.

**Network environments**

- **FR-050**: Sharing and importing MUST work in each of these environments, as long as some route
  between the two instances exists. Each MUST be covered by a validation scenario:
  1. a public IP or VPS, including a cloud VPS whose public IP is not on its own interface;
  2. a home router with NAT, with or without UPnP or NAT-PMP;
  3. carrier-grade or symmetric NAT;
  4. a network that blocks UDP;
  5. outbound traffic allowed only through an HTTP or HTTPS proxy;
  6. outbound traffic allowed only through a SOCKS5 proxy;
  7. IPv6-only or dual-stack networks;
  8. a Docker bridge network, or host networking;
  9. a TLS-intercepting corporate proxy;
  10. an isolated LAN with no internet access;
  11. instances that reach each other over a VPN, such as Tailscale or WireGuard;
  12. NAT traversal set up on the router: UPnP, NAT-PMP or PCP port mapping, or a manually
      forwarded port;
  13. a tunnelling service that can forward UDP, such as frp's UDP proxy, ZeroTier or a WireGuard
      link to a VPS;
  14. a tunnelling service that forwards only HTTP or TCP, such as Cloudflare Tunnel, ngrok or frp's
      HTTP and TCP proxies (FR-055);
  15. the user's own proxy setups from FR-051: environment-variable proxies (HTTP and SOCKS5),
      transparent and TUN-mode proxies (with fake-IP DNS, with and without UDP support), and
      router-level proxies.
- **FR-051**: Sharing, and importing above all, MUST work **without any LANrurugi-specific setup**
  when the user's machine or network already routes traffic through a proxy:
  - **Proxy environment variables**: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` (including SOCKS5)
    and `NO_PROXY` MUST be followed automatically by every outbound sharing connection.
    Local-network addresses are never sent to the proxy.
  - **The operating system's proxy setting**: followed by the native packages on platforms where
    programs do not see it through environment variables.
  - **Transparent and TUN-mode proxies**, including router-level ones and those using fake-IP DNS:
    sharing MUST keep working whatever the proxy does with UDP. When direct UDP through the proxy
    fails or is dropped, the connection MUST fall back to the encrypted TCP relay route without
    failing the import.
  - **Proxy lines that forbid UDP or P2P traffic**: the relay route and the absence of protocol
    signatures (FR-045) MUST keep imports working and unflagged.
  
  Sharing has no proxy settings of its own: the operating system's proxy is the only one it follows.
- **FR-051a**: When an importer or browser recipient reaches the sharer through a proxy, the
  address the sharer observes is the proxy's exit address. Download counting, IP restrictions and
  "first N addresses" MUST use that observed address, and the share dialog's IP-restriction help
  MUST say so.
- **FR-052**: Sharing's outbound encrypted connections MUST trust the certificate authorities the
  operating system trusts, so a network with a TLS-intercepting proxy works once its CA is
  installed in the system, with no sharing-specific setting.
- **FR-053**: Administrators MUST be able to declare the instance's external addresses, for hosts
  such as cloud VPSes whose public address is not visible on their own interfaces.
- **FR-054**: A LANrurugi rendezvous and relay node (FR-048) MUST also be runnable on its own, as
  a lightweight mode of the same program with no library and no database. Someone with a small VPS
  can then provide a node for themselves and their friends without running a full library there.
- **FR-055**: An instance's own relay role (FR-048) MUST be able to run behind an HTTP or TCP
  tunnel, or behind a reverse proxy that terminates TLS. The administrator MUST be able to set the
  public address the relay is reachable at. The instance then advertises that relay as its own
  route, so that peers reach it through the tunnel without any public relay. Automatic router
  port mapping (UPnP, NAT-PMP, PCP) MUST be switchable off. The sharing section of the settings
  MUST show whether a mapping is active, and the external address it produced.
**Tankoubon download**

- **FR-036**: Administrators MUST be able to download a whole tankoubon as one file, from the
  library context menu and from the reader page when reading inside a tankoubon. The reader MUST
  still offer downloading only the current archive.
- **FR-037**: The file MUST contain each member archive's original file, unmodified and not
  re-compressed, named with a zero-padded position prefix so that the members list in tankoubon
  order. Two members with the same title MUST NOT overwrite each other.
- **FR-038**: The download MUST start immediately and stream. It MUST NOT require a temporary copy
  of the whole tankoubon on the server, and it MUST NOT count against the temporary-space limit
  used for other generated files.
- **FR-039**: A member whose file is missing MUST NOT abort the download. The other members are
  delivered, and the file MUST include a short note naming what is missing.
- **FR-040**: Share recipients MUST be able to download a shared tankoubon in the same way when
  the share permits download. That download is part of their claim (FR-015). Everyone else who is
  not an administrator MUST be refused.

### Key Entities *(include if feature involves data)*

- **Share**: a named collection of archives and tankoubons published by one instance. Attributes:
  - its code;
  - its items;
  - permissions (any non-empty combination of import, read, download);
  - optional title;
  - download limit and used count;
  - IP restriction;
  - optional expiry;
  - embedding on/off;
  - helping allowed on/off;
  - status (active, exhausted, expired, revoked);
  - creation time.
- **Share Item**: one archive or one tankoubon inside a share. A tankoubon item carries its ordered
  member archives.
- **Share Code**: the short, human-typeable handle for a share. It resolves to the share's current
  routes and never contains a fixed address.
- **Recipient Claim**: one recipient's authorised use of a share, which is the unit counted against
  the download limit. It records:
  - the recipient's address (or addresses);
  - start and last-activity times;
  - whether it is a browser session or an import;
  - its validity, which ends on revocation or expiry.
- **Share Manifest**: what the sharer publishes for a claim: the items, their metadata, tags and
  tankoubon structure, and a fingerprint for every archive and every piece of it. Importers and
  helpers verify against it.
- **Route**: one way to reach the sharing instance at a given moment: local network, direct
  address, through both routers, or via relay.
- **Helping Record**: on a helper, one entry per share it helps or has helped. It records the
  share's last known state, reason and time, the last contact with the sharer, and the bytes
  uploaded.
- **Helper**: an instance that holds a share's content and has opted in to delivering it to other
  authorised importers.
- **Import**: on the receiving side, the set of queued downloads created from one share, plus the
  tankoubons to rebuild once its archives are present.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An administrator can go from right-clicking an archive to holding a copyable code and
  QR code in under 30 seconds, in at most three interactions with default settings.
- **SC-002**: A recipient instance can go from pasting a QR screenshot to a confirmed import in
  under 1 minute, excluding transfer time. On a phone with live scanning available, a QR code
  shown on another screen is read within 3 seconds of being in frame.
- **SC-003**: After import, 100% of shared archives carry the sharer's tags and metadata, and 100%
  of shared tankoubons have the same members in the same order.
- **SC-004**: With a download limit of N, exactly N distinct recipients can download. This holds
  when recipients start simultaneously, and pausing, resuming or retrying never consumes extra
  downloads.
- **SC-005**: A recipient outside a share's IP restriction, or holding a revoked or expired code,
  never receives any item title, cover or page from it.
- **SC-006**: A share from an instance with no public address and no port forwarding imports
  successfully from a different network in at least 95% of attempts on common home networks.
- **SC-007**: On the same local network, an import runs at no less than 80% of the throughput of a
  plain file copy between the same two machines.
- **SC-008**: An import interrupted at any point resumes without re-transferring already verified
  data, and every completed archive is byte-identical to the sharer's copy.
- **SC-009**: Revocation takes effect on new requests within 5 seconds and stops in-progress
  transfers within 30 seconds, including transfers served by helpers.
- **SC-010**: An attacker guessing default 10-character codes at the maximum allowed attempt rate
  for a year has under a 1-in-a-million chance of hitting any valid code on an instance with 100
  active shares.
- **SC-011**: With two helpers holding a share and the sharer's upload throttled, an import
  completes at least 1.5× faster than from the sharer alone.
- **SC-012**: Downloading a tankoubon starts delivering data within 2 seconds regardless of its
  size, and every member archive extracted from the download is byte-identical to the library's
  copy.
- **SC-013**: A collapsed import entry takes the same height in the queue as a single download's
  entry, whether the import has 1 archive or 500.
- **SC-014**: While a 500-archive import runs at full speed, other queued downloads still start on
  schedule, and library and reader pages on both instances respond as fast as when idle, within
  10%.
- **SC-015**: Measured over any 10-second window, sharing uploads stay within 5% of the configured
  rate caps. Scheduled switches between normal and alternative caps happen within 1 minute of the
  window boundary.
- **SC-016**: A packet capture of a complete share lifecycle (create, resolve, import over a direct
  path, import over a relay, helping) is classified by a widely used open-source traffic classifier
  as ordinary encrypted web traffic in 100% of flows. No flow is classified as BitTorrent or another
  P2P protocol, and no packet contains the strings "lanrurugi", "lrrg" or "iroh" in plaintext.
- **SC-017**: In a complete import observed end to end, outside services receive nothing but the
  encrypted knock record and end-to-end encrypted relayed bytes. Every address, manifest,
  fingerprint, authorisation and helper list reaches the importer over its connection to the
  sharer. With one reachable LANrurugi node configured and public services off, sharing between
  two NATed instances works with no connection to any other host.
- **SC-018**: For each environment in FR-050, an import between two instances in that environment
  succeeds whenever a route exists, and the validation run records which route was used. Behind a
  proxy that drops UDP, an import starts transferring within 30 seconds of being confirmed, through
  the relay route, with no setting changed in LANrurugi.

## Assumptions

- **Who shares**: "administrator" means any authenticated (password- or admin-token-holding) user.
  This project has a single administrative role, so no finer per-user sharing permission is
  introduced.
- **Who receives**: recipients do not need an account on the sharing instance, and the instance
  does not need guest mode turned on. The share code is the only credential they hold. Importing requires
  being an administrator of the *receiving* instance, because it writes to that library.
- **Default permissions**: all three are selected by default, for the least friction. The default
  can be changed in the settings page's Sharing section (FR-006a).
- **Download limit and permissions**: the download limit counts import and download claims alike.
  Reading online never counts.
- **What counts as a download**: one recipient claim, as defined in FR-015. Online reading does not
  count against the limit, because the request was to limit downloads, but it is still subject to
  the IP restriction, revocation and expiry.
- **When a share is exhausted**: browsing continues to work, because only downloading is limited.
  The share page tells the visitor downloads are no longer available.
- **What is not shared**: categories, reading progress, bookmarks and ratings are personal to the
  sharing library. Imports do not create categories, but the importer may choose a local category
  for everything imported.
- **Archive changes**: a share references library items, not frozen copies. It always serves the
  items' current files and metadata.
- **What "no signature" can and cannot mean**: FR-045 removes protocol signatures, which is what
  operators and filters act on. It cannot hide that two home addresses exchange encrypted traffic,
  when, and how much (flow analysis), nor the server names of the outside services contacted, as
  with any encrypted connection. The instance needs no HTTPS, domain or certificate of its own for
  FR-045: every outside connection is an outgoing client connection to a server with its own
  certificate, and instance-to-instance connections are encrypted with the instances' own keys.
- **Default connectivity infrastructure**: by default the instance uses public rendezvous and relay
  services that the project does not run. They hold only the encrypted knock record (FR-047), and
  relay only end-to-end encrypted bytes. They can be replaced by the instance's own or friends'
  nodes (FR-048), or turned off.
- **Why there is no public web gateway**: serving browser recipients of unreachable sharers would
  need a third-party web entry point that hands out connection information. That is excluded by
  FR-047, so those recipients import with an instance instead.
- **The irreducible minimum**: when both sides are behind NAT, they must first exchange addresses
  through someone. FR-047 limits that someone to forwarding an encrypted knock record and
  encrypted bytes. Fully serverless sharing works whenever one side is reachable.
- **QR codes**: QR codes encode the share link when the sharer has a public address, so that a
  phone camera opens the share page directly. Otherwise they encode the `lrrg:` text form for
  importing. The `lrrg:` text form is for text channels: phone cameras and most chat apps cannot
  open custom schemes, and browsers only allow `web+`-prefixed custom handlers. Decoding is
  entirely browser-side, as requested.
- **Code length**: 10 characters (50 bits) is the default. With per-address and instance-wide
  attempt limits, a year of maximal guessing against 100 active shares has under a 1-in-a-million
  chance of a hit. The optional 8-character short code is only offered together with an expiry of
  7 days or less.
- **Live camera scanning**: browsers only allow live camera access on secure connections (HTTPS or
  localhost). Many home deployments are opened over plain HTTP by local address, so photo capture
  through the device's camera app is the required fallback, not an afterthought.
- **Expiry**: optional, with no default. It was not explicitly requested, but it is the natural
  complement to revocation for brute-force and leak containment.
- **Constitution**:
  - Existing API endpoints and library data are untouched; all of this is additive (Principles I
    and II).
  - Long-lived transfers run as tasks inside the single server process (Principle III).
  - Network services the instance reaches out to are configurable and can be disabled
    (Principle V's trust-boundary spirit).
- **Out of scope**: sharing a whole category or a saved search as a live, auto-updating share;
  two-way sync between instances; sharing with a specific named user account on another instance.
