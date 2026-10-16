# bunkai_rs rationale

Rationale moved out of the source (why, invariants, SAFETY, provenance). Each heading names the item that contains the code the note describes.

## src/core.rs::module

Core sentence-boundary disambiguation pipeline.

Design notes for performance:

* `SpanRecord` borrows its `split_value` from the input text (`&'a str`)
  instead of owning a `String`, so cloning/collecting spans is a cheap
  memcpy rather than a heap allocation per record. Ownership is only
  materialised into `String`s at the public [`Span`] boundary.
* The two regexes of the original port (basic punctuators and forced line
  breaks) are replaced by hand-written scanners that are *byte-for-byte*
  faithful to what `regex` matched, including its `\s` == `\p{White_Space}`
  definition (25 codepoints, verified empirically). This removes the
  byte→char offset translation both regular expressions required.
* The emoji category table is a sorted, unique `&[(u32, Option<&str>)]`
  that is binary-searched instead of being built into a `HashMap`. Hot-path
  lookup uses a compact three-state `u8`; the public category table remains
  unchanged.
* `unify_span_annotations` sorts and dedups in place instead of building a
  `HashMap` keyed by a formatted string.

## src/core.rs::prof::enter

Stack depth at the start of a `segment_impl` call, used to fold only
that call's own frames on completion.

## src/core.rs::prof::fold

Fold the frames at/above `base` (this `segment_impl` call's own) into
the cumulative totals. Outer caller frames stay on the stack.

## src/core.rs::prof::on_segment_done

End of the public `segment()` boundary: fold the materialise frame and
report the full (segment-specific) picture.

## src/core.rs::is_ws

Unicode White\_Space — exactly what `regex`'s `\s` matched (verified over
the whole Unicode range, 25 codepoints).

All 25 White_Space codepoints are ≤ U+3000; kana/kanji/etc are above it,
so a single compare decides the overwhelmingly-common case.

## src/core.rs::SpanRecord

Internal pipeline record, kept deliberately compact: a 1-byte rule kind plus
two 32-bit offsets and the borrowed split text — 32 bytes per record, half
of the old 64 with the two `&'static str` fields. The layer path carries
these in cumulative copies, so halving the record halves that memcpy volume.

## src/core.rs::PipelineState::add_forward_rule

The end-of-text sentinel (layer 0), whose key a one-char rule span at
the very last position can collide with — see filter_previous_rule_same_span.

## src/core.rs::TextView::char_to_byte

char index → byte offset. Built lazily on first `slice` (only the
layer-producing path needs it); the boundaries-only path never touches
it, so we skip the whole allocation for `split_sentences`. Offsets are
stored as `u32` (texts beyond 4 GiB would overflow; asserted in debug).

## src/core.rs::TextView::byte_at

Build byte offsets in a single pass over the (cheap) char Vec
rather than re-decoding UTF-8 bytes.

## src/core.rs::EMOJI_NON_TARGET

Compact hot-path result for [`emoji_lookup`]. `EMOJI_NON_TARGET` includes
entries whose public category is `None` (for example skin-tone modifiers):
they belong to emoji runs, but do not make a run a target run.

## src/core.rs::emoji_lookup

Existence and target membership of an emoji codepoint, via binary search
over the sorted, unique `emoji_data::EMOJI_DATA`.

The returned `u8` is one of `EMOJI_ABSENT`, `EMOJI_NON_TARGET`, or
`EMOJI_TARGET`. The category strings are intentionally not loaded on this
hot path; the public `EMOJI_DATA` table remains available unchanged.

The table has a huge hole between the low block (`0xA9..0x3299`, 170
entries) and the plane-1 block (`0x1F004..`, 1230 entries): the whole
CJK/kanji range `0x3300..0x1F000` contains no emoji at all. Because most
Japanese prose is kana/kanji, we short-circuit that region before touching
the binary search — this is where the bulk of the annotation time went.
The region bounds are derived at compile time and asserted consistent.

A hand-written lower-bound loop instead of `slice::binary_search_by_key`
so the whole lookup inlines into the scanning hot loop.


EMOJI_DATA's minimum is U+00A9 (asserted above), so all ASCII and the
remaining C0/C1 controls can be rejected before touching the table.

Fast reject for the dominant emoji-free Japanese ranges: kana (hiragana
+ katakana) and the katakana phonetic extensions sit below 0x3300, so
they would otherwise fall into the ~170-entry low-block binary search on
every single character. The ranges hold no emoji (verified against the
table: the nearest low-block emoji are 0x3030/0x303D/0x3297/0x3299).

The entire CJK/kanji range is emoji-free — this is the hot path
for ordinary Japanese prose.

## src/core.rs::EMOJI_HIGH_LOOKUP

Direct-indexed lookup for the plane-1 emoji block: `EMOJI_HIGH_LOOKUP[cp -
0x1F000]` is the codepoint's compact three-state result, or
`EMOJI_ABSENT` if the codepoint isn't in the table. Replaces an ~11-iteration
binary search (a dependent-load chain with a ~50%-mispredicted data-dependent
branch) with a single address-known load. The table is one byte per codepoint
(~2.8 KB), and avoids the former index-plus-category loads.

## src/core.rs::str_eq

`&str` equality usable in a `const` context (`PartialEq` for `str` is not
yet const-stable). Used to precompute `EMOJI_TARGET` against the
`TARGET_EMOJI_CATEGORIES` strings.

## src/core.rs::EMOJI_TARGET_FLAGS

Per-`EMOJI_DATA`-index target flag: 1 iff the emoji's category is one of
`TARGET_EMOJI_CATEGORIES`. Precomputed so the low-block lookup does a
single `bool` test instead of a `&str` byte comparison (opportunity (5) in
`docs/optimization-hardware.md` (file not in repo)). `EMOJI_DATA`'s `&str` categories stay
untouched — they are still used by the spec model and the low/high lookups.

## src/core.rs::ScanAll

Result of the single-pass candidate scanner: every span the four
annotators (FaceMark/Emotion/Emoji/BasicRule) would find, grouped per rule
so the pipeline can merge them in the canonical rule order.

## src/core.rs::Kind::Linebreak

A whitespace run containing a newline (linebreak). Collected separately:
it is only ever turned into (start,end) spans by the Linebreak stage.

## src/core.rs::Emit

Sink for fused-scanner hits. The layer path materialises full `SpanRecord`s
(including each `split_value` slice, via the lazily built char→byte map); the
boundaries-only path merges straight into the compact `Bound` list, keeping
only the offsets Number/Dot exceptions read plus a `"."`-equals flag — so
`split_sentences` never allocates the span records or the byte map.

## src/core.rs::scan_all_with

Fused scanner: find FaceMark, EmotionExpression (words + symbols), Emoji and
BasicRule candidates in ONE pass over the character buffer, instead of
re-walking the whole text once per annotator.

It deliberately does *not* consume matched regions (other than the forward
scans inside each per-rule extractor): each position is classified once, a
rule only fires at the *start* of its run (`i == 0` or the previous char not
in that rule's trigger class), which mirrors exactly what the original
consuming finders produced — one span per maximal punct / symbol / emoji /
run, face marks and emotion words detected at every '(' / '（'. Because the
extractors are the original loops verbatim, per-position output is
byte-for-byte identical to the four separate finders (regardless of sink).

Single dispatch instead of a chain of `if`s: the trigger classes are
disjoint, so a jump table is both faster and keeps per-position
output identical (verified by fused_scanner_reproduces_original_finders (test not in repo)).

Linebreak detection, folded into the same pass: a
whitespace run is visited at its start only, and recorded
iff it contains a newline (the old separate
`find_linebreaks` pass scanned every position again for
this). Independent of the basic-rule trailing-whitespace
consumption in the original code, matching the two
separate passes exactly.

## src/core.rs::chars_range_eq

`chars[start..start + word.len()] == word` — char-vector level compare, so
the scanner never needs the byte map or a per-word allocation.

## src/core.rs::segment_impl

The fused scanner emits word/symbol emotions interleaved by position;
the original finder produced them in word-then-symbol order. The rule
keeps this exact order in its layer, so reapply the original unification
(sort + dedup) to reproduce the byte-identical EmotionExpression layer.

## src/core.rs::apply_indirect_quote

Every public layer is a snapshot of the cumulative pipeline at that
stage.  Filtering a layer in place would silently rewrite the preceding
BasicRule snapshot, so clone the compact borrowed records before the
exception stage and append the filtered result as a new layer.

## src/core.rs::is_exception_particle

Fast reject: a rule can only match if the first character after the
boundary starts one of the morpheme rules below.

## src/core.rs::chars_starts_with

Char-vector equivalent of `text[index..].starts_with(pattern)` — avoids
touching the (lazily built) char→byte map so the boundaries-only path can
check indirect-quote particles without materialising it.

## src/core.rs::unify_span_annotations

`dedup_by` receives the *later* element first; removing it keeps the
first occurrence of each run, matching the original map-insert semantics.

## src/core.rs::apply_linebreak_force

Hand-written equivalent of the old `[\n\s]*\n[\n\s]*` regex: a maximal run
of Unicode whitespace that contains at least one newline.

1:1 transform: each final span is either kept or replaced by the
linebreak run whose start equals its end; unmatched linebreaks are
appended after. Work on a copy so every previous snapshot remains
observable through the public API.

## src/core.rs::Bound

Compact span for the boundaries-only fast path used by [`split_sentences`].
Only the offsets the downstream rules actually read are kept (the
`"."`-flag NumberException checks is derived on the fly instead of being
stored). Two 32-bit offsets — 8 bytes total — keep the `active` Vec
cache-friendly; 2^32 chars (~12 GiB of Japanese text) is far beyond any
realistic input.

## src/core.rs::ByteBoundaries

Internal result that avoids allocating a one-element boundary vector when
the scanner finds no effective boundary before the end of the input.

## src/core.rs::sentence_byte_boundaries

It runs the exact same rules as [`segment_impl`], but scans the UTF-8 bytes directly (decoding inline, once) and emits compact [`Bound`]s straight out of the scanner; no per-layer records or materialisation.
First-wins-per-(start,end) semantics make the surviving set exactly the final layer's boundaries re-expressed in bytes — verified by the golden/differential suites and `tests/fast_path_consistency.rs::bounds_only_path_matches_full_pipeline`.

## src/core.rs::compute_byte_boundaries

One pass over the candidates (instead of three `retain`s): particle, dot
and number exceptions are all per-span and independent.

A span whose whole matched text is "." / "．" iff it spans exactly
that single character — byte-length equivalent of
`end == start + 1 && chars[start] ∈ {'.','．'}`.

The end-of-text sentinel is appended below instead of riding through the
pipeline as a member: it can never be rejected by an exception (its end
is the text length, its start reads nothing for the dot rules) and never
anchors a linebreak (no linebreak starts at text end), so carrying it in
`active` only added a dead element to every pass.

No newline ⇒ no LinebreakForce replacement to fold in (that path used to
build a HashMap and copy `active` even when there was nothing to
replace). Note the ends are *not* guaranteed sorted or unique even here:
spans from different rules can overlap — e.g. a kaomoji whose trailing
suffix extension swallows a following `!?…` run — so the sort+dedup below
is still required on both paths.

The merge may place a duplicate end next to a leftover linebreak, so the
(cheap, rare) sort+dedup is still required on this path.

## src/core.rs::char_at_byte

Infallible decoding of the scalar value at byte offset `b` of a valid UTF-8
slice, plus its byte length. Callers guarantee `b` is on a char boundary and
the char is fully in-bounds (the loop / run guards hold everywhere). No
Option, no `char::from_u32` validation branch — the `&str` guarantees a
valid scalar, so the unchecked conversion below is sound.

`first` gives the length directly; the codepoint is valid by construction
(the slice is a `&str`), so the indexing and unchecked conversion are
sound. One `unsafe` block keeps the doc/caller invariant in one place.
SAFETY: `b` is a char boundary within `bytes`; every multi-byte lead is
followed in-bounds by its continuation bytes (slices come from `&str`),
and `bytes` decodes from a valid UTF-8 string, so `cp` is a valid scalar.

## src/core.rs::char_at_byte_opt

Decode the scalar value at byte offset `b`, guarded: returns `None` when
`b` is past the end of the slice. Used only by the exception predicates
(rare, at span boundaries), never the hot scan loop.

## src/core.rs::prev_char_start

Byte offset of the previous char's leading byte, given a char boundary `b`.
Walks back over UTF-8 continuation bytes (at most 3).

## src/core.rs::BORING_SENTINEL

Sentinel for `prev` after a skipped boring run. Any char outside
the trigger/whitespace/emoji classes works: every per-position start-check
only tests membership in those classes, so a boring char and this sentinel are
indistinguishable to each of them (guarded by the differential suite).

## src/core.rs::ascii_skip_run

Advance over printable ASCII that cannot trigger any scanner rule.

Eight bytes are accepted at once when every lane is printable ASCII and no
lane equals one of the four ASCII trigger bytes. Treating all controls up to
space as stops is deliberately conservative: only ASCII whitespace is
semantically interesting, but the broader stop set makes the packed range
check cheap and cannot skip a trigger.

Setting every lane's high bit prevents cross-lane borrows. After
subtracting 0x21, the high bit remains set iff the byte was >= 0x21.

## src/core.rs::cjk_skip_ok

Byte-pattern predicate: is the UTF-8 scalar starting at `b` a "pure
kana/kanji" codepoint that can NEVER be a scanner trigger (basic punct /
emotion symbol), whitespace, or emoji — so it can be bulk-skipped without
decoding?

Derivation (UTF-8 byte patterns; see `docs/optimization-hardware.md` (file not in repo) for the
full write-up). A char is skippable iff its bytes are a lead `0xE4..=0xE9`
(⇒ U+4000..=0x9FFF, the CJK Unified Ideographs) or a lead `0xE3` whose 2nd
byte is `0x81..=0x83` (⇒ U+3040..=0x30FF, hiragana + katakana).

The `0xE4..=0xE9` range sits inside the emoji-free hole `0x3300..0x1F000`,
every White_Space codepoint is ≤ U+3000, and the only basic punctuators are
。=E3 80 82, ！？．=EF BC xx and ASCII `.!?`; the emotion symbols are
U+2026/2605/2606/266A. `EMOJI_DATA` has zero entries in U+4000..=0x9FFF, and
the kana range is short-circuited by `emoji_lookup` — so neither range holds
a trigger, whitespace or emoji.

Everything else is excluded because it overlaps a trigger/ws/emoji: `0xE3
0x80` (U+3000..=0x303F) holds 　U+3000 (ws), 。U+3002 (basic punct) and
〰U+3030; `0xE3 0x8A` (U+3290..) holds the emoji ⚗ U+3297 and ⚙ U+3299;
`0xEF` (fullwidth forms) holds ！U+FF01, ？U+FF1F, ．U+FF0E; `0xE0..=0xE2`
holds ws (U+2000s, U+2028/2029/202F/205F) and emotion symbols (…★☆♪);
`0xF0..=0xF4` is plane-1 emoji (e.g. 😀 = F0 9F 98 80); and `0x00..=0x7F` is
ASCII `.!?` and whitespace. The skip set is therefore provably disjoint
from every trigger/ws/emoji codepoint.

The 2nd byte exists for any lead starting a char inside a valid
`&str`; the `get` guard is defensive for a trailing partial char.

## src/core.rs::read_u64_unaligned

Read 8 little-endian bytes from `bytes[b..b+8]` without assuming
alignment. Compiles to a single unaligned load on x86-64; no `unsafe`.

## src/core.rs::cjk_skip_run

Advance `b` past a maximal run of pure kana/kanji chars (see `cjk_skip_ok`).

SWAR-accelerated. In valid UTF-8 a 3-byte lead is always followed by two
continuation bytes, so a run of skippable chars is fully described by its
*lead* bytes at offsets 0, 3, 6 (the in-between continuation bytes are
guaranteed by validity and need no check). We load 8 bytes, then compute in
parallel — via the subtract-borrow range trick — a mask of which bytes are
skippable leads, and advance by 0/3/6 (always a multiple of 3, so the next
window stays lead-aligned). A scalar tail finishes any remaining partial
window. No `unsafe` and no target-feature intrinsics (stable-only crate).

Carry-free exact byte tests: operate on the low 7 bits only, so no sum
ever exceeds 0xFE and nothing propagates between byte lanes.

byte in [0xE4,0xE9]: top bit set and low7 in [0x64,0x69].

byte == 0xE3 exactly (zero-byte test on w ^ 0xE3..).

next byte in [0x81,0x83]: top bit set and low7 in [1,3].

lead bits at byte offsets 0 / 3 / 6.

## src/core.rs::cjk_skip_run::ge

lane high bit set iff the lane's low7 value is >= k (k in 1..=0x7F).

## src/core.rs::cjk_run_is_boring

Debug-only guard: every char in `[start, end)` is genuinely boring (not a
trigger / whitespace / emoji / face-mark opener). If a future edit ever
widens `cjk_skip_ok` to include a dangerous byte pattern, this catches it
on the very next test run instead of relying on the corpus happening to
exercise the regression. Only ever invoked from a `debug_assert!`, so it is
compiled in release but never executed there.

## src/core.rs::scan_bounds_bytes

Byte-level fused scanner, mirroring [`scan_all_with`] over the char vec body
for body. Walks the UTF-8 bytes once, decoding each codepoint inline, and
pushes *byte*-offset [`Bound`]s directly — no `Vec<char>` build, no char→byte
map, no second decode pass. The dispatch and each run rule are byte-for-byte
mirrors of the char scanner, so every emit lands on a char boundary and the
surviving set is identical to the pipeline's final layer re-expressed in
bytes (guarded by `tests/fast_path_consistency.rs::bounds_only_path_matches_full_pipeline` and the golden suite).

Caches `prev.is_some_and(|p| emoji_lookup(p as u32) != EMOJI_ABSENT)` so the
dispatch doesn't re-run `emoji_lookup` on `prev` every iteration: none
of the punctuation/kaomoji trigger chars matched by the other arms are
in `EMOJI_DATA`, so `is_emoji` is exactly that predicate for every `ch`.

Bulk-skip printable ASCII and pure kana/kanji runs that cannot be a
trigger, whitespace or emoji. Dispatching on the first byte avoids
paying the CJK SWAR setup on ASCII and vice versa.

All skipped chars are non-trigger / non-ws / non-emoji, so the
sentinel below reproduces the per-position "prev" start-checks
exactly (any real boring char would pass the same checks).

Linebreak detection, folded into the same pass: a
whitespace run is visited at its start only.

## src/core.rs::is_exception_particle_bytes

Byte-level equivalents of the exception predicates; see the `TextView`
versions for the reference semantics. Reads are anchored on byte offsets
(span boundaries are char-aligned), so "previous char" means stepping back
to the previous lead byte.

## src/core.rs::byte_bound_is_dot

`end == start + 1 && chars[start] ∈ {'.','．'}` in byte terms: the span
covers exactly that single character.

## src/core.rs::apply_linebreak_force_bounds

Linebreak spans' matched text is never "." / "．".

A HashMap is overkill here: `linebreaks` already arrives from the scanner
sorted by ascending start with unique starts, so each bound's end is
looked up against the linebreak starts with a binary search — no hashing
and, apart from one tiny `consumed` byte array of length L, no heap
structure. This path runs only when the text actually contains a newline.

The `consumed` flags preserve first-wins semantics: spans from different
rules can *overlap* (a kaomoji's trailing suffix swallows a following
`!?…` run) and two bounds may therefore share the same end, but each
linebreak must anchor (replace) at most one of them. Exactly which bound
is replaced doesn't change the resulting boundary *set* (the survivor
keeps the shared end either way), so a consumed marker per linebreak —
instead of per candidate — is exact.

## src/core.rs::filter_previous_rule_same_span

Both inputs are sorted by `(start, end)`:
 • `current` is one rule's hits in scan order, which is start order;
 • `previous` is the cumulative layer, kept sorted by this same merge.
Merging instead of concatenating keeps the cumulative layer sorted, so
`apply_indirect_quote`'s later sort sees already-sorted data (pdqsort
then runs in ~O(n)). The sentinel (key == `sentinel_key`) lives only in
`previous`, sorts to the tail (largest position), and survives exactly
once — matching the old `current-filtered ++ previous` shape.

No general membership structure is needed: the four annotators' trigger
characters (`(`/`（`, the basic punctuators, `…★☆♪` and the emoji block) are
disjoint, so two different rules can never produce the same (start,end). The
only key a fresh span can collide with is the end-of-text sentinel — a
one-char rule span at the very last position occupies `(len-1, len)` — so
first-wins is exactly "skip spans occupying the sentinel's key". Fresh
spans from one rule are also internally unique, so the layer stays
byte-identical to the old full-sweep set-based version (guarded by the
golden/differential suites and the fast-bunkai cross-check).

Merge a fresh rule's spans into the cumulative layer, keeping one record per
(start,end) and the earlier rule winning.


## src/core.rs::segment

Measure only the `PipelineOutput → Segmentation` conversion here. The
`segment_impl` stages are timed inside `segment_impl` itself and folded
there, so wrapping them in this frame would double-count the whole
pipeline under the "materialize" tag.

## src/core.rs::split_sentences

Run only the boundary computation, never materialising any per-layer span
records. This is the fast path used by [`split_sentences`]: it forgoes the
cumulative layer bookkeeping, the IndirectQuote sort and the public
`Span`/`Layer` structs, which is where most of the cost of [`segment`] lives.

## src/core.rs::split_sentences_impl

Avoid allocating a boundary vector for the overwhelmingly common case
where the entire non-empty input is one sentence.

A final boundary equal to the byte length leaves the last sentence here.

## src/core.rs::is_sentence_closer

Closing punctuation that belongs to a sentence-ending quote/bracket rather
than forming a useful sentence on its own. Both Japanese/full-width and
common ASCII/typographic forms are included; consecutive closers are folded
together (`。』）」`).

## tests/public_api_e2e.rs::module

Public-API behavior that the golden/differential corpora do not pin down.

## tests/public_api_e2e.rs::TARGET_CATEGORIES

Categories whose emoji end a sentence (restated independently of the implementation).

## tests/public_api_e2e.rs::known_bug_fast_path_skips_e2_81_chars

Regression: `cjk_skip_run` once treated `E2 81 xx` (U+2049, U+205F) like kana lead bytes.

## examples/profile.rs::module

Profiling harness: run the hot path repeatedly so `perf` can sample it.
  perf record --call-graph dwarf ./target/release/examples/profile
  perf report

## tests/spec_model_cli.rs::module

E2E: runs the `spec_model` verification binary (naive formula-faithful model vs. the
optimized pipeline, golden and differential corpora) as a process; it exits non-zero on
any divergence.

## tests/golden_regression.rs::module

AUTO-GENERATED golden regression vs the original implementation.
Regenerate with scripts/generate_golden.py (see repo root). Do not edit.

## tests/differential.rs::module

AUTO-GENERATED differential test against the live Python bunkai reference.
Regenerate with `python3 scripts/generate_differential.py`; do not edit manually.

## src/emoji_data.rs::module

Auto-generated by scripts/generate_emoji_data.py (file not in repo); do not edit manually.

## Cargo.toml::features

Off by default so builds carry zero instrumentation overhead.

## tests/bunkai_reference.rs::module

Differential tests ported from the original Python `bunkai` reference suite
(https://github.com/megagonlabs/bunkai, `tests/bunkai_sbd/test_bukai_sbd.py`).

They drive the public `segment` API and check that the pipeline produces the
exact sentence splits the Python implementation is expected to produce.
Where expectations could not be taken verbatim from the reference tests,
they were verified against a live invocation of the Python reference.

## tests/bunkai_reference.rs::test_setup_sentence_variations

From setUp() of TestBunkaiSbd.test_various_inputs. None of these may
panic, and the concatenation of the produced sentences must round-trip.

## tests/bunkai_reference.rs::test_live_reference_corpus

Sentences follow the Python reference plus this crate's closing-mark
rule: a quote/bracket immediately after a boundary stays with the
preceding sentence.

## tests/bunkai_reference.rs::test_basic_rule

From tests/bunkai_sbd/annotator/test_basic_annotator.py

Punctuation runs and trailing whitespace belong to the leading sentence.

## tests/bunkai_reference.rs::test_emotion_expression

From tests/bunkai_sbd/annotator/test_emotion_expression_annotator.py

## tests/bunkai_reference.rs::test_emoji_annotator

From tests/bunkai_sbd/annotator/test_emoji_annotator.py

## tests/bunkai_reference.rs::test_emoji_full_pipeline_boundaries

`うーん` + 3×U+1F914 + `どうしよう` = 11 chars. 🤔 (Smileys) is a target
category, so the emoji run ends at char 6, and the sentinel adds char 11.

## tests/bunkai_reference.rs::test_dot_exception

From tests/bunkai_sbd/annotator/test_numeric_expression_exception_annotator.py

## tests/bunkai_reference.rs::test_number_exception

From tests/bunkai_sbd/annotator/test_number_exception_annotator.py

## tests/bunkai_reference.rs::test_facemark_subset

A representative subset of tests/bunkai_sbd/annotator/test_facemark_detector.py.
Each (text, expected_end) states the end index of the only detected facemark.

`y-~~~` is not suffix-class; verified live

HTML entities `&gt;` `&lt;` are face-symbol chars

## tests/bunkai_reference.rs::test_facemark_interior_basic_rule_punctuation

The reference lets `!` inside `(^!^)` become its own boundary, but does not
let inside-facemark text fragments with no punctuation split further.
Verified against the live Python reference.

## tests/bunkai_reference.rs::test_empty_and_short_inputs

The pipeline always ends on the LinebreakForceAnnotator layer.

## tests/bunkai_reference.rs::test_multibyte_offsets_are_char_offsets

All offsets must be in Unicode scalar values, not bytes.
"日本語です。" ends at char 6; "Englishです。" is 10 more chars → 16.

## tests/bunkai_reference.rs::test_segment_preserves_every_pipeline_layer_snapshot

This input exercises every annotator and each of the four exception
stages.  The expected snapshots are deliberately written as literals:
checking only the final boundaries would miss a stage mutating its
predecessor in place.

## tests/fast_path_consistency.rs::module

Public-API consistency: the boundaries-only fast path (`sentence_byte_boundaries` /
`split_sentences`) must equal the layer-building pipeline (`segment`) on every input.

## tests/fast_path_consistency.rs::byte_skip_boundary_safety

`scan_bounds_bytes` bulk-advances past boring ASCII and runs of pure
kana/kanji bytes (see `ascii_skip_run` and `cjk_skip_run`). The byte
ranges it skips are provably disjoint
from every trigger/whitespace/emoji codepoint, but that proof rests
on excluding the *exact* dangerous patterns: 。/　 (E3 80),
！？． (EF BC), the E3 8A emoji ⚗ U+3297 / ⚙ U+3299, and ASCII
.!?/whitespace. This test pins that invariant by placing each of
those patterns directly against long kana/kanji runs — including the
E3 8A emoji that the existing differential/golden corpus never
touches — so a wrong exclusion would change the boundary set instead
of passing by luck.

a long pure CJK run with no danger (exercises the bulk skip itself)

## tests/fast_path_consistency.rs::bounds_only_path_matches_full_pipeline

The boundaries-only fast path used by split_sentences must produce
the exact final boundaries of the layer-building pipeline. Drive
both over a deterministic corpus hitting every rule interaction.

Property check for the slicing: split_sentences must cut exactly
at the pipeline boundaries — lossless, contiguous, and every hard
cut aligned with the next boundary offset.

## examples/allocation_metrics.rs::module

Measure allocations performed by one `split_sentences` call.

Run with:

```text
cargo run --release --example allocation_metrics
```

The counters are reset after each workload has been warmed up. Formatting
and printing happen only after the counters have been read, so output from
this example cannot be included in a measurement.

## examples/allocation_metrics.rs::CountingAllocator::alloc

SAFETY: The caller supplies a valid layout, and System is the
process allocator used for the actual allocation.

## examples/allocation_metrics.rs::CountingAllocator::alloc_zeroed

SAFETY: The caller supplies a valid layout, and System is the
process allocator used for the actual allocation.

## examples/allocation_metrics.rs::CountingAllocator::dealloc

SAFETY: The pointer and layout are those returned by this allocator.

## examples/allocation_metrics.rs::CountingAllocator::realloc

SAFETY: The pointer and layout are those returned by this allocator;
the requested new size is supplied by the Vec allocation machinery.

A realloc is an allocation event from the caller's point of
view. Count the resulting size so growth costs are visible.

## examples/allocation_metrics.rs::measure

Prime lazy runtime/library state and allocator paths before resetting
the counters used for the reported call.

## benches/segment.rs::module

Benchmark the bunkai_rs sentence-boundary disambiguation pipeline.

Run with:
  cargo bench                      # everything
  cargo bench --bench segment      # just this file

The `bench` Cargo profile inherits from `[profile.release]` in Cargo.toml,
so LTO + codegen-units=1 apply to the measured build. Three text sizes are
measured for both public entry points:

* `short`  — a single sentence (~30 bytes), e.g. a chat message or a tweet.
* `medium` — ~7 KB, a typical article or chat log.
* `long`   — ~56 KB, a long document or batch of posts.

The workload matrix also separates ASCII-heavy, CJK/Kana, emoji/kaomoji,
punctuation-dense, no-boundary, sparse-newline, dense-newline, and tiny
inputs. Non-empty groups report throughput in bytes per second.

## benches/segment.rs::ascii_heavy_text

Latin-heavy text exercises the common ASCII path and the emoji lookup's
low-codepoint rejection without making the corpus completely artificial.

## benches/segment.rs::low_symbol_heavy_text

Low-codepoint symbols exercise the U+00A9..U+3299 emoji lookup block. The
corpus mixes target entries (symbols and smileys), non-target emoji entries
(objects, travel, food, and activities), absent neighbors, and ordinary
Latin-1/CJK text in UI-like notifications rather than repeating one symbol.

## benches/segment.rs::no_boundary_text

A long, terminator-free run isolates scanning cost when no boundaries are
emitted. It intentionally contains spaces and digits but no sentence mark.

## src/bin/spec_model.rs::module

Executable transcription of `docs/spec.md` (file not in repo) — the formal specification of
the bunkai_rs sentence-boundary algorithm.

This module is a deliberately **naive, formula-faithful model**: plain set
comprehensions and maximal-run scans over a `Vec<char>`, exactly as the
math defines them. It shares no code with `core.rs` — character classes,
word lists and rule tables are restated from the spec so a divergence in
either direction is caught by the comparison tests. The only shared input
is the emoji *data* table (data, not algorithm).

A standalone verification binary (`cargo run --bin spec_model`); never part of the
public API. `tests/spec_model_cli.rs` runs it as a process. See
`docs/spec.md` (file not in repo) §8 for how the verification uses it.

This file is also a standalone binary:
  cargo run  --bin spec_model   — exits non-zero on any divergence
  cargo test --test spec_model_cli — runs the binary and asserts every check passes

## src/bin/spec_model.rs::face_marks

§3.1 FaceMark `F(s)`.

## src/bin/spec_model.rs::emotion_words

§3.2 Emotion word `E_w(s)`.

## src/bin/spec_model.rs::emotion_symbols

§3.3 Emotion symbol `E_s(s)`.

## src/bin/spec_model.rs::emoji_runs

§3.4 Emoji `G(s)`.

## src/bin/spec_model.rs::basic_runs

§3.5 Basic `Q(s)`.

## src/bin/spec_model.rs::linebreaks

§3.6 Linebreak `Λ(s)` — maximal whitespace runs containing a newline.

## src/bin/spec_model.rs::is_exception_particle

§5.1 `ι(b)` — indirect-quote particle after a candidate boundary.

p(b): skip a run of newlines, capped at n−1.

## src/bin/spec_model.rs::is_exception_numeric

§5.2 `δ_num(l)` — dot between two numeric chars.

## src/bin/spec_model.rs::is_exception_mailaddress

§5.3 `δ_mail(l)` — dot between two mail chars.

## src/bin/spec_model.rs::is_exception_no_parts

§5.4 `ν(l, r)` — "No." followed by a digit.

## src/bin/spec_model.rs::spec_boundaries

The full pipeline of §6 — `B(s)` as char-index boundaries.

§4 candidate set: rule spans (sentinel-key collisions suppressed, i.e.
sentinel-first priority per Lemma 1) in the implementation's layer
order (fresh rule first, earlier layers appended).

The emotion layer is unified: sorted by key, first occurrence kept.

LinebreakForce: each linebreak (b, j) replaces the first span ending at
b (unused linebreaks are appended). Boundary set is order-invariant
(Lemma 2) — the boundary-level identity is checked by the test below.

## src/bin/spec_model.rs::corpus

The deterministic corpus: a hand-picked "bits" list, all pairs of it,
RNG concatenations, and edge cases. Same construction style as the
crate's other property suites (splitmix64, fixed seed).

## src/bin/spec_model.rs::parse_case_file

Parse the `("text", &[..])` / `("text", vec![..])` case lists embedded
in the auto-generated test files. Handles the escapes the generators
emit (`\n \t \r \0 \\ \" \' \xNN \u{..}`) and fails loudly on any
other escape. A string literal not followed by `, &[` / `, vec![`
(e.g. the `"text: {text:?}"` assert message) is skipped, not a case.

Stray literal (e.g. an assert message) — not a case.

## src/bin/spec_model.rs::try_parse_bounds_list

After a string literal, try to read `, &[` / `, vec![` plus a usize
list. `None` if what follows is not a case tuple.

## src/bin/spec_model.rs::check_generated

T4: spec model == full pipeline == fast byte path on the generated
corpus; plus T3 losslessness of `split_sentences`.

T3: lossless, contiguous, non-empty segmentation.

## src/bin/spec_model.rs::check_golden

T5: spec model reproduces the Python-derived golden corpus exactly.

## src/bin/spec_model.rs::check_differential

T5: spec model reproduces the Python-derived differential corpus.

## src/bin/spec_model.rs::check_wellformedness

T1: well-formedness of B(s) over the whole generated corpus.

## src/bin/spec_model.rs::check_first_wins

T6 + Lemma 1: rule classes are key-disjoint; the sentinel is the only
collision partner, and only at its own key.

sentinel collisions are the one allowed case

## src/bin/spec_model.rs::check_lemma2

Lemma 2: the boundary-level linebreak formula (§6 of the spec) equals
the replacement simulation on every corpus string.

Rebuild L₄ exactly as the pipeline does (§6).

(a) Simulation (as in the implementation / pipeline).

(b) Boundary-level formula of Lemma 2, over B(L₇) = ends(L₄).

## scripts/generate_differential.py::KNOWN_DIVERGENT_TEXTS

Inputs where the Rust port intentionally diverges from the Janome-based
reference, so they must not be machine-compared. Each entry names the corpus
generator tag that produces it; see build() below for the exact text.
Janome tokenises と + う as the single token とう, so the reference does NOT
apply the ["と"] indirect-quote exception here, while the dependency-free
string matcher in this port does. (See README, "Trade-offs" (section not in README).)

## tests/bunkai_reference.rs::test_various_inputs_matches_reference

Case 1
Case 2
Case 3: emoji
Case 4: repeated emoji
Case 5: facemark with slash-joy
Case 6: indirect quote particle after a newline
Case 7: bare newlines only
Case 8: mixed facemark / emoji / indirect quote
Case 9: No.1 (number exception)
Case 10: 「No.1です！」 followed by further text and indirect quote
Case 11: dangling "No." at the end

## tests/bunkai_reference.rs::test_various_inputs_expected_rules

Case 1: emotion + basic
Case 3: emotion + emoji
Case 5: emotion + basic + facemark
Case 6: linebreak (indirect quote suppressed the 「？」 boundary)
Case 7: linebreak
Case 8: basic + facemark + emoji
Case 9: No.1 → only first remains
Case 11: dangling No. → only first

## scripts/generate_golden.py::module

Regenerate tests/golden_regression.rs.

The golden file records the `final_boundaries` produced by the *original*
(pre-optimisation) bunkai_rs for a large, deterministic pseudo-random corpus,
so the optimised implementation can be proven byte-for-byte equivalent.

It works by materialising the original sources (`git show HEAD:src/...`) into a
scratch Cargo crate, building the generator binary, running it, and writing its
stdout (a self-contained Rust test) to tests/golden_regression.rs.

Requires: `cargo` and `rustc` on PATH, and this repo to be a git checkout whose
HEAD contains the pre-optimisation sources.

Usage:
    python3 scripts/generate_golden.py

## scripts/generate_differential.py::module

Regenerate `tests/differential.rs`.

Runs the live Python `bunkai` reference on a corpus and records each string's
`final_boundaries` (char offsets) as a Rust integration test. This keeps the
Rust port honest against the upstream implementation.

Setup
-----
    uv venv refenv --python 3.10
    VIRTUAL_ENV=refenv uv pip install dataclasses-json janome spans emoji emojis more_itertools toml tqdm
    git clone --depth 1 https://github.com/megagonlabs/bunkai /tmp/bunkai_ref

Usage
-----
    VIRTUAL_ENV=refenv python scripts/generate_differential.py
