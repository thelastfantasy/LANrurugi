//! Candidate generation and grouping. Pure functions only — the async pass in `mod.rs` feeds them.

use std::collections::{HashMap, HashSet};

use super::model::{ArchiveMeta, GroupRecord, PairEvidence};

pub const ALGORITHM: &str = "cover-phash-v1";
pub const DEFAULT_THRESHOLD: u32 = 7;

/// The largest threshold the 8×8 pigeonhole banding can support: at 8 bits, two hashes 7 bits
/// apart must still share one band, but 8 apart need not. Anything higher used to degrade to
/// enumerating every pair (n²/2 `String` pairs — gigabytes on a real library), so the API rejects
/// it instead.
pub const MAX_THRESHOLD: u32 = DEFAULT_THRESHOLD;

/// A single band bucket larger than this is not expanded: pairwise work inside one bucket is
/// quadratic, and a bucket that big means the banding cannot discriminate (e.g. thousands of
/// archives sharing one band). Skipped buckets are logged; the pass keeps the pairs it did see.
const MAX_BUCKET: usize = 512;

/// Pairs within `threshold` bits, found through 8 pigeonhole bands (8 bits each): any two hashes
/// differing in at most 7 bits must share at least one band byte exactly, so no near pair is
/// missed. A threshold of 8 or more degrades to all pairs, where banding proves nothing.
pub fn candidates(hashes: &HashMap<String, [u8; 8]>, threshold: u32) -> Vec<(String, String)> {
    let ids: Vec<&String> = hashes.keys().collect();
    let mut out: HashSet<(String, String)> = HashSet::new();
    // Above the banding limit no band is guaranteed to be shared, so no pair is provably near —
    // return nothing rather than falling back to enumerating every pair (n²/2, gigabytes on a real
    // library). The API rejects such thresholds before they get here.
    if threshold > MAX_THRESHOLD {
        return Vec::new();
    }

    let mut buckets: HashMap<(u8, u8), Vec<&String>> = HashMap::new();
    for id in &ids {
        let hash = &hashes[*id];
        for band in 0..8u8 {
            buckets
                .entry((band, hash[band as usize]))
                .or_default()
                .push(id);
        }
    }
    for ((band, _), members) in &buckets {
        if members.len() > MAX_BUCKET {
            tracing::warn!(
                band,
                members = members.len(),
                "duplicate scan skipped an oversized cover-hash bucket"
            );
            continue;
        }
        for (i, a) in members.iter().enumerate() {
            for b in members.iter().skip(i + 1) {
                out.insert(ordered(a, b));
            }
        }
    }
    out.into_iter().collect()
}

fn ordered(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// The evidence for one pair, or `None` when metadata contradicts a visual match.
pub fn evidence(
    a: &ArchiveMeta,
    b: &ArchiveMeta,
    distance: u32,
    threshold: u32,
) -> Option<PairEvidence> {
    let creators_a = a.creators();
    let creators_b = b.creators();
    let creator_conflict = !creators_a.is_empty()
        && !creators_b.is_empty()
        && !creators_a.iter().any(|c| creators_b.contains(c));

    let title_similarity = title_similarity(&a.title, &b.title);
    let same_creator = !creators_a.is_empty() && creators_a.iter().any(|c| creators_b.contains(c));

    // A different creator entirely is a different work, however similar the cover looks (shared
    // template covers, re-used stock art). Same creator is evidence on its own; otherwise the
    // titles have to agree.
    if creator_conflict {
        return None;
    }
    if !same_creator && title_similarity < 0.55 {
        return None;
    }

    let pages_ratio = ratio(a.pages, b.pages);
    if a.pages > 0 && b.pages > 0 && pages_ratio < 0.8 {
        return None;
    }
    let size_ratio = ratio_u64(a.size, b.size);
    if a.size > 0 && b.size > 0 && size_ratio < 0.2 {
        return None;
    }

    Some(PairEvidence {
        a: a.id.clone(),
        b: b.id.clone(),
        distance,
        similarity: 1.0 - f64::from(distance) / 64.0,
        title_similarity,
        pages_ratio,
        size_ratio,
        shared_tags: a
            .tags
            .iter()
            .filter(|t| b.tags.contains(t))
            .cloned()
            .collect(),
        chained: distance > threshold,
    })
}

fn ratio(a: u32, b: u32) -> f64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if hi == 0 {
        1.0
    } else {
        f64::from(lo) / f64::from(hi)
    }
}

fn ratio_u64(a: u64, b: u64) -> f64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if hi == 0 {
        1.0
    } else {
        lo as f64 / hi as f64
    }
}

/// Trigram Jaccard over the release-marker-cleaned, lowercased, alphanumeric-only title.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let a = normalize_title(a);
    let b = normalize_title(b);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let ga = trigrams(&a);
    let gb = trigrams(&b);
    let inter = ga.intersection(&gb).count();
    let union = ga.union(&gb).count();
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

fn normalize_title(title: &str) -> String {
    let cleaned = lanrurugi_scanner::release_markers::clean_title(title);
    cleaned
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn trigrams(s: &str) -> HashSet<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 3 {
        return HashSet::from([s.to_string()]);
    }
    chars.windows(3).map(|w| w.iter().collect()).collect()
}

/// Components of the accepted pairs, as full records ready to store.
pub fn build_groups(
    metas: &[ArchiveMeta],
    hashes: &HashMap<String, [u8; 8]>,
    threshold: u32,
    now: i64,
    rejects: &HashSet<(String, String)>,
) -> Vec<GroupRecord> {
    let by_id: HashMap<&str, &ArchiveMeta> = metas.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut parent: HashMap<&str, &str> = metas
        .iter()
        .map(|m| (m.id.as_str(), m.id.as_str()))
        .collect();

    fn find<'a>(parent: &mut HashMap<&'a str, &'a str>, x: &'a str) -> &'a str {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != cur {
            let next = parent[cur];
            parent.insert(cur, root);
            cur = next;
        }
        root
    }

    let mut accepted: Vec<PairEvidence> = Vec::new();
    for (a, b) in candidates(hashes, threshold) {
        let (Some(ma), Some(mb)) = (by_id.get(a.as_str()), by_id.get(b.as_str())) else {
            continue;
        };
        // A pair the user ruled out is never an edge again, whatever group it now falls into:
        // the decision is global, not tied to the member list it was made against.
        let ordered = if a <= b {
            (a.clone(), b.clone())
        } else {
            (b.clone(), a.clone())
        };
        if rejects.contains(&ordered) {
            continue;
        }
        let distance = super::cover::distance(&hashes[&a], &hashes[&b]);
        if distance > threshold {
            continue;
        }
        let Some(pair) = evidence(ma, mb, distance, threshold) else {
            continue;
        };
        let ra = find(&mut parent, ma.id.as_str());
        let rb = find(&mut parent, mb.id.as_str());
        if ra != rb {
            parent.insert(ra, rb);
        }
        accepted.push(pair);
    }

    let mut components: HashMap<&str, Vec<String>> = HashMap::new();
    let ids: Vec<&str> = metas.iter().map(|m| m.id.as_str()).collect();
    for id in ids {
        let root = find(&mut parent, id);
        components.entry(root).or_default().push(id.to_string());
    }

    let mut groups = Vec::new();
    for (_, members) in components {
        if members.len() < 2 {
            continue;
        }
        let member_set: HashSet<&str> = members.iter().map(String::as_str).collect();
        let pairs: Vec<PairEvidence> = accepted
            .iter()
            .filter(|p| member_set.contains(p.a.as_str()) && member_set.contains(p.b.as_str()))
            .cloned()
            .collect();
        let (chained, middle) = chain_marker(&members, &pairs, hashes, threshold);
        let mut members = members;
        members.sort();
        groups.push(GroupRecord {
            algorithm: ALGORITHM.to_string(),
            threshold,
            built_at: now,
            members,
            pairs,
            chained,
            middle,
            ..Default::default()
        });
    }
    groups.sort_by(|a, b| a.members.first().cmp(&b.members.first()));
    groups
}

/// A group is chained when two of its members are further apart than the threshold (they belong
/// together only through a third file). The middle member is the one closest to both ends.
fn chain_marker(
    members: &[String],
    pairs: &[PairEvidence],
    hashes: &HashMap<String, [u8; 8]>,
    threshold: u32,
) -> (bool, Option<String>) {
    let mut worst: Option<(u32, &str, &str)> = None;
    for (i, a) in members.iter().enumerate() {
        for b in members.iter().skip(i + 1) {
            let d = super::cover::distance(&hashes[a], &hashes[b]);
            if d > threshold && worst.map(|(wd, _, _)| d > wd).unwrap_or(true) {
                worst = Some((d, a, b));
            }
        }
    }
    let Some((_, a, b)) = worst else {
        return (false, None);
    };
    let goal = (super::cover::distance(&hashes[a], &hashes[b]) as f64 / 2.0).ceil() as u32;
    let mut best: Option<(u32, &String)> = None;
    for m in members {
        if m == a || m == b {
            continue;
        }
        let left = super::cover::distance(&hashes[m], &hashes[a]);
        let right = super::cover::distance(&hashes[m], &hashes[b]);
        if left > threshold || right > threshold {
            continue;
        }
        let score = left.max(right).abs_diff(goal);
        if best.map(|(s, _)| score < s).unwrap_or(true) {
            best = Some((score, m));
        }
    }
    let _ = pairs;
    (true, best.map(|(_, m)| m.clone()))
}

/// The member that wins the most pairwise comparisons. A pair is won by the closer cover, then by
/// the one with more pages, then the larger file — the same order of evidence the reviewer is told
/// to weigh. Used for the card's "best copy" marker (spec FR-003).
pub fn best_member(
    group: &GroupRecord,
    pages: &HashMap<String, u32>,
    sizes: &HashMap<String, u64>,
) -> Option<String> {
    let mut wins: HashMap<&str, u32> = HashMap::new();
    for pair in &group.pairs {
        let (a, b) = (pair.a.as_str(), pair.b.as_str());
        let winner = if pair.similarity > 0.0 && pair.distance < 64 {
            // A pair always has one side that the metadata gate liked better; the tie-breakers are
            // page completeness (a missing page cannot be recovered) and then file size.
            let pa = pages.get(a).copied().unwrap_or(0);
            let pb = pages.get(b).copied().unwrap_or(0);
            if pa != pb {
                if pa > pb {
                    a
                } else {
                    b
                }
            } else {
                let sa = sizes.get(a).copied().unwrap_or(0);
                let sb = sizes.get(b).copied().unwrap_or(0);
                if sa >= sb {
                    a
                } else {
                    b
                }
            }
        } else {
            a
        };
        *wins.entry(winner).or_default() += 1;
    }
    let mut ranked: Vec<(&&str, &u32)> = wins.iter().collect();
    ranked.sort_by(|a, b| {
        let pages_of = |id: &str| pages.get(id).copied().unwrap_or(0);
        let size_of = |id: &str| sizes.get(id).copied().unwrap_or(0);
        b.1.cmp(a.1)
            .then_with(|| pages_of(b.0).cmp(&pages_of(a.0)))
            .then_with(|| size_of(b.0).cmp(&size_of(a.0)))
    });
    ranked.first().map(|(id, _)| id.to_string())
}

/// Rebuilds groups after user rejections removed some edges: a member whose only edges were
/// rejected is no longer part of any group, and a chained group can split in two.
pub fn regroup_by_pairs(
    record: &GroupRecord,
    hashes: &HashMap<String, [u8; 8]>,
) -> Vec<GroupRecord> {
    let mut parent: HashMap<&str, &str> = record
        .members
        .iter()
        .map(|id| (id.as_str(), id.as_str()))
        .collect();
    fn find<'a>(parent: &mut HashMap<&'a str, &'a str>, x: &'a str) -> &'a str {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != cur {
            let next = parent[cur];
            parent.insert(cur, root);
            cur = next;
        }
        root
    }
    for pair in &record.pairs {
        let (Some(a), Some(b)) = (parent.get(pair.a.as_str()), parent.get(pair.b.as_str())) else {
            continue;
        };
        let (a, b) = (*a, *b);
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    }
    let mut components: HashMap<&str, Vec<String>> = HashMap::new();
    for id in &record.members {
        let root = find(&mut parent, id.as_str());
        components.entry(root).or_default().push(id.clone());
    }
    let mut out = Vec::new();
    for (_, members) in components {
        if members.len() < 2 {
            continue;
        }
        let set: HashSet<&str> = members.iter().map(String::as_str).collect();
        let pairs: Vec<PairEvidence> = record
            .pairs
            .iter()
            .filter(|p| set.contains(p.a.as_str()) && set.contains(p.b.as_str()))
            .cloned()
            .collect();
        let (chained, middle) = chain_marker(&members, &pairs, hashes, record.threshold);
        let mut members = members;
        members.sort();
        out.push(GroupRecord {
            algorithm: record.algorithm.clone(),
            threshold: record.threshold,
            built_at: record.built_at,
            members,
            pairs,
            chained,
            middle,
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, title: &str, tags: &[&str], pages: u32, size: u64) -> ArchiveMeta {
        ArchiveMeta {
            id: id.to_string(),
            title: title.to_string(),
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            pages,
            size,
        }
    }

    fn hash_of(byte: u8) -> [u8; 8] {
        [byte; 8]
    }

    #[test]
    fn an_oversized_threshold_does_not_enumerate_every_pair() {
        // Thresholds above 7 cannot be handled by 8 bands, and the old fallback enumerated n(n-1)/2
        // pairs — gigabytes on a real library. The API rejects them; the function must not silently
        // agree to the enumeration either.
        let mut hashes = HashMap::new();
        for i in 0..64u8 {
            hashes.insert(format!("{i:040}"), [i; 8]);
        }
        assert!(candidates(&hashes, 8).is_empty());
        assert!(candidates(&hashes, 200).is_empty());
    }

    #[test]
    fn an_oversized_bucket_is_skipped() {
        // Every archive with the same band byte in the same position would otherwise form one
        // quadratic bucket; the pass skips it instead of allocating the pair set.
        let mut hashes = HashMap::new();
        for i in 0..600u16 {
            // Same value in every band, so every bucket is over the cap and none is expanded.
            hashes.insert(format!("{i:040}"), [0xAB; 8]);
        }
        assert!(candidates(&hashes, 7).is_empty());
    }

    #[test]
    fn near_hashes_are_candidates() {
        let mut hashes = HashMap::new();
        let mut near = [0u8; 8];
        near[0] = 0b0000_0001; // one bit away from all-zero
        hashes.insert("a".to_string(), [0u8; 8]);
        hashes.insert("b".to_string(), near);
        hashes.insert("c".to_string(), [0xff; 8]);
        let pairs = candidates(&hashes, 7);
        assert!(pairs.contains(&("a".to_string(), "b".to_string())));
        assert!(!pairs
            .iter()
            .any(|(x, y)| (x == "c" && y == "a") || (x == "a" && y == "c")));
    }

    #[test]
    fn identical_covers_with_same_title_group() {
        let metas = vec![
            meta("a", "Work [DL版]", &["artist:someone"], 100, 1000),
            meta("b", "Work [中国翻訳]", &["artist:someone"], 99, 800),
        ];
        let hashes = HashMap::from([("a".to_string(), hash_of(0)), ("b".to_string(), hash_of(0))]);
        let groups = build_groups(&metas, &hashes, 7, 1, &HashSet::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members, vec!["a".to_string(), "b".to_string()]);
        assert!(!groups[0].chained);
    }

    #[test]
    fn different_creator_is_rejected_even_with_same_cover() {
        let metas = vec![
            meta("a", "Work", &["artist:alice"], 100, 1000),
            meta("b", "Work", &["artist:bob"], 100, 1000),
        ];
        let hashes = HashMap::from([("a".to_string(), hash_of(0)), ("b".to_string(), hash_of(0))]);
        assert!(build_groups(&metas, &hashes, 7, 1, &HashSet::new()).is_empty());
    }

    #[test]
    fn chained_group_reports_the_middle() {
        // a..b differ in 4 bits, b..c in 4 bits, a..c in 8 (over the threshold of 7).
        let mut ha = [0u8; 8];
        ha[0] = 0b0000_0000;
        let mut hb = [0u8; 8];
        hb[0] = 0b0000_1111; // 4 bits from a
        let mut hc = [0u8; 8];
        hc[0] = 0b1111_1111; // 8 bits from a, 4 from b
        let hashes = HashMap::from([
            ("a".to_string(), ha),
            ("b".to_string(), hb),
            ("c".to_string(), hc),
        ]);
        let metas = vec![
            meta("a", "Work", &["artist:alice"], 100, 1000),
            meta("b", "Work 2", &["artist:alice"], 100, 900),
            meta("c", "Work 3", &["artist:alice"], 100, 800),
        ];
        let groups = build_groups(&metas, &hashes, 7, 1, &HashSet::new());
        assert_eq!(groups.len(), 1);
        assert!(groups[0].chained);
        assert_eq!(groups[0].middle.as_deref(), Some("b"));
    }

    #[test]
    fn the_best_member_wins_on_pages_then_size() {
        let metas = vec![
            meta("a", "Work", &["artist:alice"], 100, 1000),
            meta("b", "Work", &["artist:alice"], 120, 1000),
            meta("c", "Work", &["artist:alice"], 120, 2000),
        ];
        let hashes = HashMap::from([
            ("a".to_string(), hash_of(0)),
            ("b".to_string(), hash_of(0)),
            ("c".to_string(), hash_of(0)),
        ]);
        let group = build_groups(&metas, &hashes, 7, 1, &HashSet::new()).remove(0);
        let pages = HashMap::from([
            ("a".to_string(), 100u32),
            ("b".to_string(), 120),
            ("c".to_string(), 120),
        ]);
        let sizes = HashMap::from([
            ("a".to_string(), 1000u64),
            ("b".to_string(), 1000),
            ("c".to_string(), 2000),
        ]);
        assert_eq!(best_member(&group, &pages, &sizes).as_deref(), Some("c"));
    }

    #[test]
    fn regrouping_drops_members_whose_only_edges_were_rejected() {
        let metas = vec![
            meta("a", "Work", &["artist:alice"], 100, 1000),
            meta("b", "Work", &["artist:alice"], 100, 1000),
            meta("c", "Work", &["artist:alice"], 100, 1000),
        ];
        let hashes = HashMap::from([
            ("a".to_string(), hash_of(0)),
            ("b".to_string(), hash_of(0)),
            ("c".to_string(), hash_of(0)),
        ]);
        let mut group = build_groups(&metas, &hashes, 7, 1, &HashSet::new());
        assert_eq!(group.len(), 1);
        let mut record = group.remove(0);
        // The user says a..b is not a duplicate; c was only connected through b.
        record
            .pairs
            .retain(|p| !(p.a == "a" && p.b == "b") && !(p.a == "b" && p.b == "a"));
        let split = regroup_by_pairs(&record, &hashes);
        assert!(split.iter().all(|g| g.members.len() >= 2));
        assert!(split.iter().any(|g| g.members.contains(&"c".to_string())));
    }

    #[test]
    fn title_similarity_ignores_release_markers() {
        assert!(title_similarity("Work [DL版]", "Work [中国翻訳]") > 0.9);
        assert!(title_similarity("Work", "Totally Different") < 0.2);
    }
}
