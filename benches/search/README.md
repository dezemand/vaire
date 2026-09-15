# Search benchmark

A search-quality + latency benchmark for `vaire search` (issue #52: `vaire search` ranks
long spec documents above short, more relevant concept/principle nodes). This is the one
benchmark every ranking-optimisation branch runs identically, so results are comparable
across branches.

```
cargo bench --bench search -- [flags]
```

It builds one or more corpora into fresh tempdirs, indexes each with
`vaire::index::build::run(..., Mode::Full)`, runs each corpus's queries through
`vaire::search::search`, and prints Markdown metrics + latency tables to stdout. A full
JSON report (everything, including every query's top-10 hits) is written to
`target/search-bench/<label>.json` for later `--compare`.

The harness is built ONLY on `vaire`'s stable public API
(`vaire::search::search`, `vaire::index::{db, build}`, `vaire::config::Config`,
`vaire::corpus::Repo`, `vaire::embed::{Embedder, from_user_config}`,
`vaire::userconfig::UserConfig`) so it keeps working unmodified as the search
implementation changes on other branches. It never reaches the network for embeddings
(the default `local` embedder is fully in-process) and never reads `credentials.toml`
unless you explicitly select `--embedder openai`.

## What it measures

For every query with at least one relevance judgment (see the query-file format below):

- `first_rel_rank` — 1-based rank of the first result graded >= 1 in the top 10 (absent if
  none appear).
- `primary_rank` — 1-based rank of the first result graded `2` (the primary answer).
- **RR@10** — `1 / first_rel_rank` (0 if absent).
- **nDCG@10** — gain `2^grade - 1`, discount `log2(rank + 1)`, ideal DCG computed from the
  judgments themselves.
- **Recall@10** — `|relevant ∩ top10| / |relevant|`.
- **Success@1** / **Success@3** — whether `first_rel_rank` is within the top 1 / top 3.
- **Primary@1** — whether the primary (`grade == 2`) answer is the very first hit.
- **DocIntrusion@1** — `1` iff the top-1 hit has type `document` and that document is not
  itself judged relevant for the query, else `0`. This is issue #52's symptom, measured
  directly: a long spec document crowding out the concept/principle a query is actually about.
  (Lower is better.)

A query with no judgments at all (`[query.relevant]` omitted or empty) is excluded from
every quality metric above, but is still timed (see Latency).

Every metric is averaged overall and per query `category`.

**Latency**: one untimed warm-up pass over every query, then each query is run `--repeat`
times (default 5) and the *median* of those timings is its latency. The reported
mean/p50/p95/max are computed over that per-query median array, in milliseconds to 3
decimals. Index build wall time (ms), node count, and section count are reported per
corpus alongside latency.

## Corpora

Select with `--corpus public|external|scale`, comma-separated, or `all` (default:
`public`). `all` expands to `public` + `scale`, plus `external` only when both its inputs
(below) are actually supplied.

### `public`

The corpus checked into this repo, built from:

- Every `.md` under `benches/search/data/public/nodes/` (preserved relative paths) —
  hand-authored fixture nodes with real judgments.
- Generated `document` nodes from this repo's own long-form docs (`spec/design.md`,
  `spec/cli.md`, `spec/registry.md`, `spec/manifest.md`, `README.md`, `CHANGELOG.md`) —
  any leading `---`-fenced frontmatter in the source is stripped and replaced with vaire
  frontmatter (`id`, `type: document`, `name`).
- Generated `skill` nodes, one per `skills/<dir>/SKILL.md`.

If `benches/search/data/public/nodes/` is missing or empty, the corpus is still built (from
the generated documents/skills only) and a notice is printed — useful before the fixture
data exists yet, and for the `--queries` smoke-test flow below.

Queries: `benches/search/data/public/queries.toml` (see the format below). If that file
doesn't exist yet, the corpus still builds and indexes; quality metrics are skipped with a
printed notice (latency reports zero queries).

```
cargo bench --bench search -- --corpus public
```

### `external` — your own corpus, never checked in

Point the harness at any Vairë package on disk you don't want committed to this repo — a
private or internal knowledge base, a client's corpus, anything with its own
`knowledge.toml` and your own judgments file kept outside version control. Provide:

- `--external-dir <dir>` (or env `VAIRE_BENCH_EXTERNAL_DIR`) — the package root. It is
  copied into a tempdir (excluding `.git/`, `.vaire/`, `node_modules/`) and indexed with
  its own manifest; **your source directory is never written to**.
- `--external-queries <file>` (or env `VAIRE_BENCH_EXTERNAL_QUERIES`) — your own judgments
  file, in the same TOML format as `public`'s queries.
- `--external-name <label>` (default `external`) — the label recorded in reports and
  printed tables. **The directory path itself is never recorded anywhere** (not in the
  Markdown, not in the JSON report), so results can be shared without revealing where your
  corpus lives.

Both `--external-dir` and `--external-queries` (or their env-var equivalents) are
required; if either is missing, the corpus is skipped with a one-line notice — including
when it's only implicitly requested via `--corpus all`.

```
export VAIRE_BENCH_EXTERNAL_DIR=/path/to/your/corpus
export VAIRE_BENCH_EXTERNAL_QUERIES=/path/to/your/judgments.toml
cargo bench --bench search -- --corpus external --external-name my-corpus
```

### `scale` — deterministic synthetic corpus for latency

A synthetic corpus generated in-process (`--scale-nodes N`, default 3000) with an inline
SplitMix64-seeded xorshift64* PRNG and a **fixed seed**, so it is byte-identical across
machines, branches, and runs — no data file, no network. Vocabulary: ~20k synthetic
pronounceable words with Zipf-like frequency, mixed with ~40 common English stopwords.
~85% are short `concept` nodes (1-3 `##` sections, 50-300 words); ~15% are long `document`
nodes (15-40 sections, 200-600 words *each*).

30 "needle" concept nodes are planted: each has a distinctive 2-3 word mid-frequency-
vocabulary name, repeated several times in its own short body. Those same words are then
injected many times across a few randomly-chosen long documents — reproducing issue #52's
adversarial case directly: a long document's raw term-count-summed-over-every-section can
dwarf a short, genuinely relevant node's score.

Queries: one `name`-category query per needle (judged: the needle's own id, grade `2`),
plus 30 random 1-6 word queries with no judgments at all (category `latency` — excluded
from every quality metric, but still timed).

```
cargo bench --bench search -- --corpus scale --scale-nodes 500
```

## Query-file format

TOML, one `[[query]]` table per query:

```toml
[[query]]
id = "name-loose-end"        # unique, stable
text = "loose end"
category = "name"            # name | alias | keyword | question | paraphrase | document | filter
type = "concept"             # optional -> SearchOpts.type_filter
note = "optional"
[query.relevant]
"concept:loose-end" = 2      # 2 = primary answer, 1 = also relevant
"skill:vaire-files" = 1
```

Validated on load: every `id` unique and non-empty, every judgment grade in `1..=2`. After
indexing, every judged id that does not actually exist in the built index is reported
loudly (printed to stderr and recorded as a report notice) — a stale or typoed judgment,
never silently ignored.

A query with no `[query.relevant]` table at all carries no judgments (see the synthetic
corpus's `latency` category above): it contributes nothing to quality metrics but is still
measured for latency.

## Embedders

`--embedder local|cached|openai` (default `local`).

- **`local`** — `vaire::embed::from_user_config(&UserConfig::default())`, the crate's
  built-in in-process embedder (`EmbeddingConfig::default()` sets `provider = Local`, so
  this is fully offline). Unless `--embedder openai` is selected, `VAIRE_CONFIG_HOME` is
  pointed at a fresh empty temp dir at startup, so the benchmark never reads (or is
  influenced by) your real `~/.config/vaire`.
- **`cached`** — vectors come only from `--vector-cache <file>` (required); any miss is a
  hard error reporting how many texts were missing. No network, no live embedder at all.
- **`openai`** — `vaire::embed::from_user_config(&UserConfig::load()?)`, the real network
  embedder. **Requires `--vector-cache <file>`.** Cache hits skip the network; a miss is
  embedded by the inner embedder (truncated to 8000 bytes first, backed off to a UTF-8
  char boundary — the OpenAI embedder otherwise sends sections as-is and the API rejects
  oversized inputs) and the cache file is written back with the newly-embedded vectors.
  The cache *key* is always the sha256 of the **full, untruncated** text, so a later exact
  lookup (from indexing or another run) still hits. Never invoked by this repo's own CI or
  acceptance runs — implemented for completeness, per the benchmark's spec.

Cache file shape (`--vector-cache`):

```json
{
  "identity": "<inner embedder's identity()>",
  "dims": 384,
  "vectors": { "<sha256 hex of the full text>": [0.1, 0.2, "..."] }
}
```

## Comparing two branches

Build each branch's report, then diff them — no rebuilding on `--compare`, it only reads
two JSON files:

```
# on branch A (e.g. main)
cargo bench --bench search -- --corpus public,scale --label main

# on branch B (your ranking change)
cargo bench --bench search -- --corpus public,scale --label my-fix

# compare
cargo bench --bench search -- --compare target/search-bench/main.json target/search-bench/my-fix.json
```

`--compare` prints, per corpus: overall + per-category metric tables as
`base -> new (delta)`, a latency delta table, and per-query primary-rank changes split into
improved / regressed lists (matched by query `id`; a query present in only one side is
called out rather than silently dropped).

## Other flags

- `--queries <file>` — override the queries file for a single selected corpus (so
  `--corpus` must resolve to exactly one corpus). Meant for smoke tests: point it at a
  throwaway file (**outside** `benches/search/data/`, e.g. a `mktemp -d` directory) judging
  a couple of ids you know the corpus generates, such as `document:cli-spec` or
  `document:design-spec` for the `public` corpus's generated documents. If the path doesn't
  exist, this is an error (unlike a corpus's own default queries file, which skips
  gracefully).
- `--repeat N` — timed repeats per query (default 5).
- `--scale-nodes N` — node count for the `scale` corpus (default 3000).
- `--out <dir>` — where JSON reports are written (default `target/search-bench`).
- `--label <string>` — report label; also the JSON filename (sanitized to a flat,
  filesystem-safe name). Default `<branch>-<shortsha>` from `git`, else `unlabeled`.
- `--verbose` — also print the per-query Markdown table (id, category, query text
  truncated to 50 chars, primary expected id, primary rank, first relevant rank, top-1 id,
  nDCG@10).

`cargo bench` always appends `--bench` to a bench binary's own arguments (custom harness or
not); the harness ignores that token.

## Smoke-testing without fixture data

`cargo bench --bench search -- --corpus public` works even before
`benches/search/data/public/nodes/` or `queries.toml` exist — it builds the corpus from the
generated documents/skills alone and skips quality metrics with a notice. To exercise the
full scoring path before real fixture data lands, write a throwaway queries file **outside**
`benches/search/data/` and point `--queries` at it:

```
dir=$(mktemp -d)
cat > "$dir/smoke.toml" <<'EOF'
[[query]]
id = "smoke-cli-spec"
text = "command line interface specification"
category = "keyword"
[query.relevant]
"document:cli-spec" = 2
EOF
cargo bench --bench search -- --corpus public --queries "$dir/smoke.toml" --verbose
```

## Running the sanity test

`tests/search_relevance.rs` builds the `public` corpus with the `local` embedder, runs its
queries (skipping gracefully with a printed notice if `queries.toml` doesn't exist yet),
and prints the same Markdown tables:

```
cargo test --test search_relevance -- --nocapture
```

Its assertions are deliberately weak for now (every metric finite and within `[0, 1]`, at
least one query loaded when the file exists) — real thresholds land once there is a
ranking change on another branch to hold to a standard.
