#![cfg(feature = "pack")]

//! `vaire pack` — the artifact contract (registry.md §11): committed-tree inputs, the
//! publication gate, attachment integrity, the exported index, and reproducibility.

mod common;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use common::Corpus;
use vaire::commands::pack;
use vaire::error::VaireError;
use vaire::index::Index;
use vaire::index::db::SCHEMA_VERSION;

/// Read every entry of a `.tgz` artifact into `name → bytes`.
fn entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let file = std::fs::File::open(path).expect("artifact exists");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut out = BTreeMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let name = entry.path().expect("entry path").display().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("entry bytes");
        out.insert(name, bytes);
    }
    out
}

/// Unpack the artifact's `.vaire/index.db` into a temp dir and open it.
fn artifact_index(artifact: &Path) -> (tempfile::TempDir, Index) {
    let map = entries(artifact);
    let (_, db) = map
        .iter()
        .find(|(name, _)| name.ends_with(".vaire/index.db"))
        .expect("artifact carries an index");
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("index.db");
    std::fs::write(&db_path, db).expect("write index");
    let index = Index::open(&db_path).expect("artifact index opens");
    (dir, index)
}

#[test]
fn packs_the_fixture_with_manifest_corpus_and_index() {
    let c = Corpus::fixture();
    let out = pack::run(&c.ctx(), false).expect("pack");

    assert_eq!(out.name, "test-corpus");
    assert_eq!(out.version, "0.1.0");
    assert_eq!(out.commit, common::head(c.root()));
    assert_eq!(out.artifact, ".vaire/dist/test-corpus-0.1.0.tgz");

    let artifact = c.root().join(&out.artifact);
    let map = entries(&artifact);
    assert_eq!(out.entries, map.len());

    let top = "test-corpus-0.1.0/";
    assert!(
        map.keys().all(|k| k.starts_with(top)),
        "one top-level directory: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    for expected in [
        "knowledge.toml",
        ".vaire/index.db",
        "knowledge/entities/people/jane-doe.md",
        "projects/atlas/2026_q2/meeting-notes/2026-06-10-broker-sync.md",
    ] {
        assert!(
            map.contains_key(&format!("{top}{expected}")),
            "missing {expected}"
        );
    }
    // The corpus file bytes are the committed bytes, verbatim.
    assert_eq!(
        map[&format!("{top}knowledge.toml")],
        std::fs::read(c.root().join("knowledge.toml")).unwrap()
    );

    // The digest in the output is the digest of the file on disk.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    std::io::copy(&mut std::fs::File::open(&artifact).unwrap(), &mut hasher).unwrap();
    assert_eq!(out.sha256, format!("{:x}", hasher.finalize()));
}

#[test]
fn the_artifact_index_is_current_schema_portable_and_cacheless() {
    let c = Corpus::fixture();
    let out = pack::run(&c.ctx(), false).expect("pack");
    let (_dir, index) = artifact_index(&c.root().join(&out.artifact));

    assert_eq!(index.schema_version(), Some(SCHEMA_VERSION));
    assert_eq!(
        index.meta("package_name").unwrap().as_deref(),
        Some("test-corpus")
    );
    assert_eq!(
        index.meta("index_source").unwrap().as_deref(),
        Some("committed")
    );
    assert_eq!(
        index.meta("last_indexed_commit").unwrap(),
        Some(common::head(c.root()))
    );
    // The artifact records its own *format*, and nothing about the build that produced it.
    // The digest of these bytes is what a lockfile pins and what `push` re-derives from a
    // tag, so anything varying with the packing binary would make the same release hash
    // differently after a `vaire upgrade` — and a re-push then report it as somebody
    // else's version.
    assert_eq!(
        index.meta("artifact_format").unwrap().as_deref(),
        Some(vaire::index::export::ARTIFACT_FORMAT)
    );
    assert_eq!(
        index.meta("packed_by").unwrap(),
        None,
        "the packing vaire's identity is not part of the artifact"
    );

    let nodes = index.scalar_i64("SELECT count(*) FROM nodes", ()).unwrap() as usize;
    assert_eq!(nodes, out.nodes);
    assert!(nodes > 0);
    // The machine-local embedding cache never ships.
    assert_eq!(
        index
            .scalar_i64("SELECT count(*) FROM embed_cache", ())
            .unwrap(),
        0
    );
    // Embeddings ship by default.
    let embedded = index
        .scalar_i64("SELECT count(*) FROM embeddings", ())
        .unwrap() as usize;
    assert_eq!(embedded, out.embeddings);
    assert!(embedded > 0, "default pack ships vectors");
}

#[test]
fn no_embeddings_strips_vectors_but_keeps_sections() {
    let c = Corpus::fixture();
    let out = pack::run(&c.ctx(), true).expect("pack");
    assert_eq!(out.embeddings, 0);
    let (_dir, index) = artifact_index(&c.root().join(&out.artifact));
    assert_eq!(
        index
            .scalar_i64("SELECT count(*) FROM embeddings", ())
            .unwrap(),
        0
    );
    assert!(
        index
            .scalar_i64("SELECT count(*) FROM sections", ())
            .unwrap()
            > 0,
        "prose still ships; only vectors are stripped"
    );
}

#[test]
fn packing_the_same_commit_twice_is_byte_identical() {
    let c = Corpus::fixture();
    let first = pack::run(&c.ctx(), false).expect("pack once");
    let first_bytes = std::fs::read(c.root().join(&first.artifact)).unwrap();
    let second = pack::run(&c.ctx(), false).expect("pack twice");
    let second_bytes = std::fs::read(c.root().join(&second.artifact)).unwrap();
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(first_bytes, second_bytes);
}

#[test]
fn archive_metadata_is_pinned() {
    let c = Corpus::fixture();
    let out = pack::run(&c.ctx(), false).expect("pack");
    // The commit's own timestamp — the only mtime the archive is allowed to carry.
    let commit_epoch: u64 = {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(c.root())
            .args(["show", "-s", "--format=%ct", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    };

    // The gzip container's own header must carry no timestamp either: bytes 4..8 of a
    // gzip stream are its little-endian MTIME field. Asserted on the raw bytes so the
    // check cannot depend on any decoder's header-parsing behavior.
    let raw = std::fs::read(c.root().join(&out.artifact)).unwrap();
    assert_eq!(
        &raw[4..8],
        &[0, 0, 0, 0],
        "gzip header mtime is untimestamped"
    );

    let file = std::fs::File::open(c.root().join(&out.artifact)).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut names = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        assert_eq!(header.uid().unwrap(), 0);
        assert_eq!(header.gid().unwrap(), 0);
        assert_eq!(header.mode().unwrap(), 0o644);
        assert_eq!(
            header.mtime().unwrap(),
            commit_epoch,
            "entry mtime is the commit epoch"
        );
        names.push(entry.path().unwrap().display().to_string());
    }
    assert!(!names.is_empty());
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "entries are emitted in sorted order");
}

#[test]
fn a_diverged_manifest_refuses_to_pack() {
    let c = Corpus::fixture();
    let manifest = c.root().join("knowledge.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("description = \"uncommitted edit\"\n");
    std::fs::write(&manifest, text).unwrap();

    let err = pack::run(&c.ctx(), false).unwrap_err();
    match err {
        VaireError::Pack(msg) => assert!(msg.contains("differs between HEAD"), "{msg}"),
        other => panic!("expected Pack error, got {other:?}"),
    }
}

#[test]
fn no_commits_refuses_to_pack() {
    let c = Corpus::empty(); // git repo, manifest written, nothing committed
    let err = pack::run(&c.ctx(), false).unwrap_err();
    match err {
        // The manifest is not committed either, but "no commits" is the first honest gate.
        VaireError::Pack(msg) => assert!(msg.contains("no commits"), "{msg}"),
        other => panic!("expected Pack error, got {other:?}"),
    }
}

#[test]
fn a_dirty_working_tree_warns_but_packs() {
    let c = Corpus::fixture();
    c.add("knowledge/entities/uncommitted.md", "not committed\n");
    let out = pack::run(&c.ctx(), false).expect("pack");
    assert!(
        out.warnings.iter().any(|w| w.contains("uncommitted")),
        "{:?}",
        out.warnings
    );
    // The uncommitted file is not in the artifact.
    let map = entries(&c.root().join(&out.artifact));
    assert!(
        !map.keys().any(|k| k.contains("uncommitted.md")),
        "committed tree only"
    );
}

#[test]
fn check_violations_block_packing() {
    let c = Corpus::fixture();
    // Two files claiming the same composed ID — the duplicate-entity guard.
    c.add(
        "knowledge/entities/dup-a.md",
        "---\nid: dup\ntype: system\nname: Dup A\n---\n# Dup A\n",
    )
    .add(
        "knowledge/entities/dup-b.md",
        "---\nid: dup\ntype: system\nname: Dup B\n---\n# Dup B\n",
    )
    .commit();

    let err = pack::run(&c.ctx(), false).unwrap_err();
    assert!(
        matches!(err, VaireError::CheckViolations(_)),
        "expected CheckViolations, got {err:?}"
    );
}

#[test]
fn a_broken_relative_link_blocks_packing() {
    let c = Corpus::fixture();
    c.add(
        "knowledge/notes.md",
        "# Notes\n\nSee ![wiring](../attachments/missing.png).\n",
    )
    .commit();

    let err = pack::run(&c.ctx(), false).unwrap_err();
    match err {
        VaireError::Pack(msg) => {
            assert!(msg.contains("broken relative link"), "{msg}");
            assert!(msg.contains("attachments/missing.png"), "{msg}");
            assert!(msg.contains("no such file at HEAD"), "{msg}");
        }
        other => panic!("expected Pack error, got {other:?}"),
    }
}

#[test]
fn an_exclude_glob_vetoes_a_referenced_file() {
    let c = Corpus::fixture();
    // drafts/ is excluded by the default globs: a stray link must not republish a
    // draft. The author gets a warning, and the file stays out of the artifact.
    c.add("knowledge/drafts/wip.md", "# WIP\n")
        .add("knowledge/pointer.md", "[wip](drafts/wip.md)\n")
        .commit();

    let out = pack::run(&c.ctx(), false).expect("pack");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("drafts/wip.md") && w.contains("veto")),
        "{:?}",
        out.warnings
    );
    let map = entries(&c.root().join(&out.artifact));
    assert!(
        !map.keys().any(|k| k.contains("drafts/wip.md")),
        "the veto is real: referenced or not, an excluded file does not ship"
    );
}

#[test]
fn directory_links_resolve_against_packed_files() {
    let c = Corpus::fixture();
    // `entities/` is satisfied by the packed fixture files under it; `../reference/`
    // exists at HEAD but nothing under it is packed (warning); `../nowhere/` does not
    // exist at all (violation).
    c.add("reference/raw-source.md", "raw, unindexed source\n")
        .add(
            "knowledge/toc.md",
            "[entities](entities/) and [sources](../reference/)\n",
        )
        .commit();
    let out = pack::run(&c.ctx(), false).expect("dir links to packed content are fine");
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("reference/") && w.contains("nothing under it ships")),
        "{:?}",
        out.warnings
    );
    // A directory link is not an inclusion demand: nothing under reference/ ships.
    let map = entries(&c.root().join(&out.artifact));
    assert!(!map.keys().any(|k| k.contains("raw-source.md")));

    c.add("knowledge/toc.md", "[gone](../nowhere/)\n").commit();
    let err = pack::run(&c.ctx(), false).unwrap_err();
    match err {
        VaireError::Pack(msg) => assert!(msg.contains("no such directory at HEAD"), "{msg}"),
        other => panic!("expected Pack error, got {other:?}"),
    }
}

#[test]
fn referenced_markdown_ships_transitively_as_payload_not_nodes() {
    let c = Corpus::fixture();
    // guides/ is outside the corpus globs. Referencing setup.md pulls it in as
    // payload; its own image link is chased too — a shipped document never carries
    // broken links. Neither becomes a node: only the globs decide what is corpus,
    // even when the payload file carries id: frontmatter.
    c.add(
        "guides/setup.md",
        "---\nid: setup\ntype: system\nname: Setup\n---\n# Setup\n\n![diagram](diagram.png)\n",
    )
    .add("guides/diagram.png", "PNGDATA\n")
    .add(
        "knowledge/howto.md",
        "# Howto\n\nFollow the [setup guide](../guides/setup.md).\n",
    )
    .commit();

    let out = pack::run(&c.ctx(), false).expect("pack");
    let map = entries(&c.root().join(&out.artifact));
    assert!(map.contains_key("test-corpus-0.1.0/guides/setup.md"));
    assert!(
        map.contains_key("test-corpus-0.1.0/guides/diagram.png"),
        "the closure is transitive through referenced Markdown"
    );
    let (_dir, index) = artifact_index(&c.root().join(&out.artifact));
    assert_eq!(
        index
            .scalar_i64("SELECT count(*) FROM nodes WHERE path LIKE 'guides/%'", ())
            .unwrap(),
        0,
        "payload is never corpus: globs alone decide what gets an id"
    );
}

#[test]
fn gitignored_targets_warn_instead_of_failing() {
    let c = Corpus::fixture();
    // The togaf pattern: raw source material retained locally, declared local-only by
    // the package's own .gitignore, never committed — and the README points at it.
    c.add(".gitignore", "local-notes/\n").add(
        "knowledge/sources.md",
        "See [notes](../local-notes/) and [one](../local-notes/raw.md).\n",
    );
    c.commit();

    let out = pack::run(&c.ctx(), false).expect("gitignored targets do not block");
    let gitignored: Vec<&String> = out
        .warnings
        .iter()
        .filter(|w| w.contains("gitignored"))
        .collect();
    assert_eq!(gitignored.len(), 2, "{:?}", out.warnings);
}

#[test]
fn referenced_files_ship_verbatim_and_unreferenced_files_never_ship() {
    let c = Corpus::fixture();
    // Co-located next to the entity that uses it — no reserved directory. Clearly
    // binary bytes (NULs, invalid UTF-8) prove the git blob path is byte-safe.
    let wiring: Vec<u8> = vec![0xFF, 0xD8, 0x00, 0x01, 0xFE, 0x00, 0x42];
    std::fs::create_dir_all(c.root().join("knowledge/entities")).unwrap();
    std::fs::write(c.root().join("knowledge/entities/wiring.png"), &wiring).unwrap();
    std::fs::write(
        c.root().join("knowledge/entities/unused.bin"),
        b"never linked",
    )
    .unwrap();
    c.add(
        "knowledge/entities/line-3.md",
        "# Line 3\n\n![wiring](wiring.png)\n",
    )
    .commit();

    let out = pack::run(&c.ctx(), false).expect("pack");
    let map = entries(&c.root().join(&out.artifact));
    assert_eq!(
        map["test-corpus-0.1.0/knowledge/entities/wiring.png"], wiring,
        "referenced bytes survive git and tar untouched"
    );
    // Inclusion is by reference: an unlinked file is not an orphan to warn about —
    // it simply does not ship.
    assert!(!map.keys().any(|k| k.contains("unused.bin")));
    assert!(
        !out.warnings.iter().any(|w| w.contains("unused.bin")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn reference_style_definitions_are_extracted_too() {
    let c = Corpus::fixture();
    let photo = b"\x00\x01photo".to_vec();
    std::fs::create_dir_all(c.root().join("knowledge")).unwrap();
    std::fs::write(c.root().join("knowledge/photo.jpg"), &photo).unwrap();
    c.add(
        "knowledge/gallery.md",
        "# Gallery\n\nSee the ![photo][p].\n\n[p]: photo.jpg\n",
    )
    .commit();

    let out = pack::run(&c.ctx(), false).expect("pack");
    let map = entries(&c.root().join(&out.artifact));
    assert_eq!(map["test-corpus-0.1.0/knowledge/photo.jpg"], photo);
}
