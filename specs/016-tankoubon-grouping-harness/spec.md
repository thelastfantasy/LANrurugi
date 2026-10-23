# Feature Specification: Local + LLM Harness for Tankoubon Grouping Suggestions

**Feature Branch**: `016-tankoubon-grouping-harness`

**Created**: 2026-10-20

**Status**: Draft

**Input**: User description: "单行本智能分组的本地+LLM 联合 harness：现有「AI 智能创建单行本」完全是本地打分，会把不同作者、仅共享「中国翻訳」「DL版」等发布信息和语言/汉化等通用标签的作品推荐成一组。改造为联合流程：本地打分做召回；确定性门槛提升准确率；LLM 复核当前显示的候选组，只能剔除成员；记录分数明细、AI 判断与用户操作作为标注；用标注构建离线评估集；界面可查看为什么分到一组。"

## Context

The library's "smart create tankoubon" action suggests groups of loose archives that look like one
series. Today it is a purely local similarity judgment: a blend of title-embedding similarity, tag
overlap and title-word overlap, accepted above one threshold, then grouped so every pair in a group
clears it. It runs without any AI provider.

It over-groups in a specific, recognizable way. Two works by different creators are suggested as one
series because both titles carry the same release markers ("Chinese translation", "DL edition",
"uncensored") and both carry the tags nearly every archive in a translated library has (language,
"translated"). Nothing counts *against* a pair: different creators are not evidence of a different
series, so shared boilerplate alone can carry a pair over the threshold.

The user asked for this to become a harness — a pipeline whose stages can be inspected, measured and
improved over time — rather than another round of hand-tuned weights. The pipeline is: local scoring
proposes (recall), deterministic gates reject what is provably not one series, an AI reviewer judges
the groups actually shown (precision), every decision is recorded with its evidence, the user's own
accept/reject actions become labels, and any future change is measured against those labels before
it ships.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Release markers and generic tags no longer make a series (Priority: P1)

A user opens smart tankoubon creation. Works by different creators that only share release markers
and library-wide common tags are no longer offered as one series, while genuine series — the same
creator's numbered volumes, the same cosplayer's sets, a known anthology's issues — are still offered.

**Why this priority**: it removes the exact failure the user reported, needs no AI provider, costs
nothing per use, and benefits every user including those without an API key.

**Independent Test**: on a library containing two works by different creators whose titles both
carry "[Chinese translation] [DL edition]" and whose tags are otherwise generic, plus three numbered
volumes by one creator, open the suggestions. Delivered value: the cross-creator pair is not
suggested; the three volumes are.

**Acceptance Scenarios**:

1. **Given** two archives whose creators are both known and share no creator in common, and neither
   is an anthology, **When** suggestions are computed, **Then** they are never placed in the same
   group, however high their other similarity.
2. **Given** two archives whose titles differ except for release markers, **When** their title-word
   overlap is measured, **Then** the release markers contribute nothing.
3. **Given** two archives sharing only tags that most of the library carries, **When** their tag
   overlap is measured, **Then** that overlap counts for little; sharing a rare tag counts for much
   more.
4. **Given** archives that were correctly suggested together before this change (same creator's
   volumes, same cosplayer's sets, anthology issues), **When** suggestions are computed, **Then** they
   are still suggested together.

---

### User Story 2 - An AI reviewer checks the group in front of the user (Priority: P2)

When a suggested group is shown, an AI reviewer has already judged which of its members really belong
to one series. Members it rejects arrive unticked, each with a short reason; the user can still tick
them back. The reviewer can only remove members — it can never add an archive the local stage did
not propose.

**Why this priority**: deterministic gates catch provable mismatches; the reviewer catches the
semantic ones (same creator, unrelated works; a coincidental title word) that no rule can express.
It is the precision stage of the pipeline.

**Independent Test**: with an API key configured, open a suggestion containing one member from an
unrelated work. Delivered value: that member is unticked with a reason, the rest stay ticked, and
creating the tankoubon uses only the ticked members.

**Acceptance Scenarios**:

1. **Given** a group being shown and an API key configured, **When** the reviewer judges it,
   **Then** every member is marked kept or rejected, rejected members are unticked by default with a
   reason, and the user may tick any member back before creating.
2. **Given** the reviewer's answer names an archive that was not in the group, or omits a member,
   **When** the answer is checked, **Then** it is rejected, the reviewer is asked once more with the
   specific defect, and if that also fails the group is shown exactly as the local stage proposed it.
3. **Given** no API key, a provider failure, or a timeout, **When** a group is shown, **Then** it is
   shown as the local stage proposed it, with a visible note that it was not reviewed — never a
   blocked or empty card.
4. **Given** the user pages through suggestions, **When** a group comes into view, **Then** it has
   been reviewed already or is reviewed then; groups the user never reaches are never reviewed.
5. **Given** the reviewer rejects all but one member of a new-group suggestion, **When** it is
   shown, **Then** the card says the reviewer found no series here and creation stays possible only
   by the user ticking members back.
6. **Given** a group the local evidence already settles (every member shares one creator, or one
   cosplayer), **When** it comes into view, **Then** it is not sent for review at all, and the card
   says why.
7. **Given** a group is sent for review, **When** its input is assembled, **Then** each member is
   represented only by its cleaned title, its creators and a handful of its most distinctive tags —
   never its full tag list — and the whole input stays within a fixed size.

---

### User Story 3 - The user can see why a group was suggested (Priority: P3)

Each suggestion can show its evidence: how similar the titles are, which tags overlap and how rare
they are, whether the creators match, and the reviewer's verdict per member.

**Why this priority**: it lets the user decide quickly, and it is what makes a wrong suggestion
diagnosable instead of mysterious.

**Independent Test**: open the evidence for any suggestion. Delivered value: the shared creator or
cosplayer, the strongest shared tags and the reviewer's reasons are visible without leaving the card.

**Acceptance Scenarios**:

1. **Given** a suggestion, **When** the user opens its evidence, **Then** they see, per member pair
   or per member, the title similarity, the overlapping tags with their rarity, and whether the
   creators match.
2. **Given** a reviewed suggestion, **When** the evidence is shown, **Then** each member's verdict
   and reason are shown alongside the scores.

---

### User Story 4 - The user's decisions become labels (Priority: P4)

Every suggestion that is shown is recorded with its evidence and the reviewer's verdict. What the
user then does — create it, untick members, tick back members the reviewer rejected, or choose "don't
suggest again" — is recorded against that suggestion as labels.

**Why this priority**: without labels nothing in the pipeline can be measured, so no later change can
be shown to be an improvement.

**Independent Test**: create one suggested tankoubon with one member unticked, and dismiss another
suggestion. Delivered value: the stored labels say which member pairs the user confirmed as one
series and which they rejected.

**Acceptance Scenarios**:

1. **Given** a suggestion the user creates with some members unticked, **When** labels are recorded,
   **Then** every pair of ticked members is a positive label and every ticked–unticked pair is a
   negative label.
2. **Given** a suggestion the user dismisses with "don't suggest again", **When** labels are
   recorded, **Then** its member pairs are negative labels.
3. **Given** a member the reviewer rejected that the user ticked back, **When** labels are recorded,
   **Then** it is recorded as a reviewer disagreement in addition to the pair labels.
4. **Given** the user only pages past a suggestion, **When** nothing is decided, **Then** no label is
   recorded for it.

---

### User Story 5 - Changes are measured before they ship (Priority: P5)

A maintainer changing weights, thresholds, gates or the reviewer's instructions can replay the
pipeline against the recorded labels and see precision and recall for the local stage and for the
whole pipeline, compared with the current settings.

**Why this priority**: it is what turns "tune and hope" into an engineering loop, and it is the
reason for building this as a harness; it has no user-visible effect on its own.

**Independent Test**: with a set of recorded labels, run the evaluation for the current settings and
for a modified setting. Delivered value: a report of precision and recall for each, from labels
alone, without changing the library.

**Acceptance Scenarios**:

1. **Given** recorded labels, **When** the evaluation runs, **Then** it reports, for the local stage
   and for the full pipeline, how many labelled positive pairs were proposed together (recall) and
   how many proposed pairs were labelled positive (precision).
2. **Given** a candidate change, **When** the evaluation runs for both the current and the candidate
   settings, **Then** both reports appear side by side with their differences.
3. **Given** the evaluation includes the reviewer stage, **When** it runs, **Then** it reuses
   recorded verdicts where the inputs are unchanged and only calls the provider for groups it has not
   seen, and states how many provider calls it made.
4. **Given** the evaluation runs, **When** it finishes, **Then** nothing in the library, the labels
   or the user's suggestions has changed.

---

### Edge Cases

- One archive has a known creator and the other has none: no conflict is assumed; unknown is not
  evidence of a different creator.
- Two archives share one creator among several (a collaboration): they do not conflict.
- An anthology and a single-creator work: the creator gate does not apply when either side is an
  anthology, as today's category rule already treats anthologies specially.
- A "circle" on one side and an "artist" on the other: circle and artist values are compared as one
  creator set, so a circle work and its member artist's solo work can still match.
- A cosplayer-tagged pair whose artists differ: a shared cosplayer stays a strong positive signal, as
  today; the creator gate does not override it.
- A suggestion that adds to an existing tankoubon: the reviewer judges each proposed new member
  against the tankoubon's existing members, and only proposed new members can be rejected.
- The reviewer rejects a member the user later ticks back and creates: that disagreement is a label
  the evaluation can use against the reviewer.
- The library changes while the modal is open (a member deleted elsewhere): a review for a group
  whose members no longer exist is discarded rather than shown.
- A library where almost every archive carries the same tag set: rarity weighting makes those tags
  near-worthless, so title evidence and creator agreement decide.
- No labels yet: the evaluation reports that it has nothing to measure rather than a meaningless
  100%.

## Requirements *(mandatory)*

### Functional Requirements

**Deterministic gates (US1)**

- **FR-001**: Two archives whose creator sets (artist and circle values together) are both non-empty
  and disjoint MUST NOT be placed in the same suggested group, unless either is an anthology or the
  two share a cosplayer.
- **FR-002**: Release markers (translation, distribution-edition and censorship markers, page or
  file-format markers) MUST be removed from titles before title-word overlap is measured. The marker
  list MUST be one maintained list, not scattered literals.
- **FR-003**: Tag overlap MUST weight each shared tag by how rare it is in the library, so a tag most
  archives carry contributes little and a rare shared tag contributes much.
- **FR-004**: Groups that are genuine series under today's behaviour (same creator, same cosplayer,
  anthology issues) MUST still be suggested.

**AI review (US2)**

- **FR-005**: A suggested group MUST be reviewable by an AI reviewer that returns, for each member, a
  kept/rejected verdict and a short reason.
- **FR-006**: The reviewer's answer MUST be checked deterministically: it must name exactly the
  group's members, each once. A failing answer MUST get at most one repair attempt carrying the
  specific defect; a second failure MUST fall back to the unreviewed group.
- **FR-007**: The reviewer MUST NOT be able to add an archive to a group; it can only reject members.
- **FR-008**: Review MUST happen only for the group in view and at most the next two, so opening the
  modal never reviews every suggestion. A group's review MUST be reused, not repeated, while its
  members are unchanged.
- **FR-009**: Without an API key, or when the review fails or exceeds its time bound, the group MUST
  be shown as proposed locally, marked as not reviewed. The local suggestions MUST never wait on the
  reviewer to appear.
- **FR-010**: Rejected members MUST be shown unticked with their reason, and the user MUST be able to
  tick them back.
- **FR-011**: Reviews MUST go through the project's single model-call path, attributed to their own
  feature in the usage ledger, bounded and cancellable like every other AI call.
- **FR-021**: Local filtering MUST shrink what is sent before any review: each member is sent as its
  title with release markers removed and truncated to a fixed length, its creators, and at most a
  fixed number of its rarest tags; generic and noise tags (language, translation status, dates,
  sources, ratings) are never sent. One review's input MUST stay within a fixed size.
- **FR-022**: A group whose members all share one creator, or all share one cosplayer, MUST skip
  review: the local evidence settles it and spending tokens on it adds nothing.
- **FR-023**: A group larger than a fixed member cap MUST send only that many members — the ones
  with the weakest local evidence — and keep the rest as proposed locally.

**Evidence (US3)**

- **FR-012**: Every suggestion MUST carry its evidence: per member pair, title similarity, rarity-
  weighted tag overlap with the strongest shared tags, title-word overlap and creator agreement; plus
  the reviewer's verdicts when reviewed.
- **FR-013**: The evidence MUST be viewable from the suggestion card without leaving it.

**Records and labels (US4)**

- **FR-014**: Every suggestion shown MUST be recorded with its members, evidence, the settings that
  produced it, and the reviewer's verdict and the instructions version used.
- **FR-015**: Creating a suggested group MUST record pair labels: ticked–ticked positive,
  ticked–unticked negative; reviewer disagreements MUST be recorded as such.
- **FR-016**: "Don't suggest again" MUST record negative pair labels for that suggestion's members.
- **FR-017**: Labels MUST persist across restarts and MUST NOT expire on a timer; they are the
  evaluation's ground truth. They MUST be deletable as a whole by the user.

**Evaluation (US5)**

- **FR-018**: A maintainer MUST be able to run an evaluation that reports precision and recall over
  the recorded labels, for the local stage and for the full pipeline.
- **FR-019**: The evaluation MUST accept candidate settings (weights, threshold, gates on/off,
  reviewer instructions) and report them side by side with the current settings.
- **FR-020**: The evaluation MUST NOT change library data, labels or suggestions, and MUST reuse
  recorded reviewer verdicts for unchanged inputs, reporting how many provider calls it made.

### Key Entities

- **Candidate group**: members proposed by the local stage, with pairwise evidence.
- **Pair evidence**: title similarity, rarity-weighted tag overlap and its strongest shared tags,
  title-word overlap, creator agreement, the gate that blocked the pair if any.
- **Review**: per-member verdict and reason for one group, the instructions version, the outcome
  class (reviewed, repaired, fell back, not configured).
- **Suggestion record**: a shown suggestion, its evidence, its review and the settings that produced
  it.
- **Pair label**: two archives, positive or negative, where it came from (created, unticked,
  dismissed) and when.
- **Evaluation report**: precision and recall per stage and per settings variant, with the label
  count and provider calls made.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 0 suggested groups contain two members whose known creators are disjoint (outside the
  anthology and shared-cosplayer exceptions), on any library.
- **SC-002**: On a fixture library containing genuine series and boilerplate-only look-alikes, the
  look-alike pairs are suggested 0 times and the genuine series are all still suggested.
- **SC-003**: The first suggestion appears no later than it does today; review never delays it.
- **SC-004**: Opening the modal and viewing N groups costs at most N + 2 reviews, and 0 reviews
  without an API key; groups settled by local evidence cost 0.
- **SC-008**: One review's input stays under a fixed token budget (on the order of 1,500 input tokens
  for a ten-member group), visible per call in the AI usage panel.
- **SC-005**: In 100% of reviews, the members shown are a subset of the members proposed locally.
- **SC-006**: Every created or dismissed suggestion produces labels; a maintainer can obtain a
  precision/recall report for two settings variants from the labels in under one minute for a
  library of 10,000 archives, with no change to the library.
- **SC-007**: On the user's own labelled suggestions, the full pipeline's precision is higher than
  the local stage's alone, as measured by that report.

## Assumptions

- **Creators** are the `artist:` and `circle:` tag values. "Unknown" (no such tags) is never
  evidence of a mismatch.
- **Anthology** is recognised the way the grouping already recognises it (`category:anthology`).
- **Rarity** is computed from the library's own tag frequencies at suggestion time; no external
  corpus.
- **The reviewer sees** titles and tags of a group's members — the same kind of content the existing
  rename suggestions already send to the provider — and nothing else.
- **Labels are local** to this instance and single-user, like the rest of the library.
- **The evaluation is a maintainer tool** (a command), not a UI. Showing its report in the product is
  out of scope.
- **Automatic tuning is out of scope**: the evaluation measures; a human decides what to change.
- **Existing "don't suggest again" behaviour is kept**: an ignored combination stays hidden as
  today; this feature only adds labels on top.
- **The modal's paging is kept**: one group at a time, as today.
