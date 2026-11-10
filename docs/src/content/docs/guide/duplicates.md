---
title: Duplicate review
description: How duplicate archives are detected, compared and pruned.
---

# Duplicate review

LANrurugi's duplicate page groups archives that look like the same work, so you can keep one copy
and delete the rest without hunting through the library.

## How a group is found

1. **Cover, perceptually.** Every archive's cover thumbnail is reduced to a 64-bit perceptual hash.
   Two covers within a few bits of each other are candidates. Unlike a hash of the image *bytes*,
   this survives re-encoding, resizing and a slightly different crop — which is exactly the case a
   re-scan produces.
2. **Metadata, to reject false positives.** Candidates are only kept when the release-marker-cleaned
   titles agree (or the artist/circle tags match), page counts are within 20% of each other, and
   file sizes are within a factor of five. Two different creators are never grouped, however alike
   the covers look.
3. **Grouping.** Candidates form connected components. A group can be *chained*: A matches B and B
   matches C, while A and C differ beyond the threshold. Chained groups are labelled, and the middle
   member is marked so you can see why the group holds together.

The scan runs as a background job (30% of the cores at most) and is incremental: cover hashes are
cached per archive id, and the id is itself a content fingerprint, so a cached hash can never belong
to different bytes.

## The suggestion

A model reviews each group once. It sees text and numbers only — titles, artist/circle, language,
pages, sizes — never the images, because the hash already answered "do these look alike". It decides
the relationship (same work, different editions, adjacent volumes, unrelated) and which copies to
keep, with a one-line reason.

If no model is configured, or the answer does not pass validation, the card says **local check** and
keeps the copies with the most pages. Nothing is deleted either way.

## Deciding

Every member starts checked (keep). Unchecking marks a copy to discard; the buttons below the card
apply the decision:

- **Smart compare** — side-by-side pages, with each side's sharpness and the pages only one side has.
- **Keep the N selected** — saves the current selection.
- **Keep suggested** — applies the model's suggestion.
- **Not duplicates** — records the pair as unrelated; it will not be grouped again, even after a
  rescan (captured by id, so a replaced file is a new question).
- **Delete the N unselected** — asks for confirmation and then deletes those archives, including
  their category, tankoubon and reading-progress entries.

## Cautions

- Detection is heuristic: a chained group can join two genuinely different works through a shared
  cover template. The comparison view exists for exactly that case.
- Deleting is permanent; there is no undo beyond your normal backups.
- The old `/duplicates` data (`LRR_DUPLICATE_GROUPS`) is still written, so third-party tools that read
  it keep working.
