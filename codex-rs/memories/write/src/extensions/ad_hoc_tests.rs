use super::*;
use crate::memory_extensions_root;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[tokio::test]
async fn seeds_instructions_and_preserves_custom_file() {
    let codex_home = TempDir::new().expect("create temp codex home");
    let memory_root = codex_home.path().join("memories");
    let instructions_path = memory_extensions_root(&memory_root).join("ad_hoc/instructions.md");

    seed_instructions(&memory_root)
        .await
        .expect("seed ad-hoc instructions");

    let seeded = tokio::fs::read_to_string(&instructions_path)
        .await
        .expect("read seeded ad-hoc instructions");
    assert_eq!(seeded, INSTRUCTIONS);
    assert!(seeded.contains("<memory_mutation_policy>"));
    assert!(seeded.contains("Do not retain a"));
    assert!(seeded.contains("tombstone"));

    tokio::fs::write(&instructions_path, "custom instructions")
        .await
        .expect("write custom instructions");
    seed_instructions(&memory_root)
        .await
        .expect("seed ad-hoc instructions again");

    assert_eq!(
        tokio::fs::read_to_string(&instructions_path)
            .await
            .expect("read custom ad-hoc instructions"),
        "custom instructions"
    );
}

#[tokio::test]
async fn propagates_errors_reading_existing_instructions() {
    let codex_home = TempDir::new().expect("create temp codex home");
    let memory_root = codex_home.path().join("memories");
    let instructions_path = memory_extensions_root(&memory_root).join("ad_hoc/instructions.md");
    tokio::fs::create_dir_all(&instructions_path)
        .await
        .expect("create directory at instructions path");

    let err = seed_instructions(&memory_root)
        .await
        .expect_err("directory should not be accepted as instructions file");
    let direct = tokio::fs::read_to_string(&instructions_path)
        .await
        .expect_err("reading a directory should fail");

    assert_eq!(err.kind(), direct.kind());
}

#[tokio::test]
async fn concurrent_initial_seeding_is_idempotent() {
    let codex_home = TempDir::new().expect("create temp codex home");
    let memory_root = codex_home.path().join("memories");
    let (left, right) = tokio::join!(
        seed_instructions(&memory_root),
        seed_instructions(&memory_root)
    );

    left.expect("first concurrent seed");
    right.expect("second concurrent seed");
    assert_eq!(
        tokio::fs::read_to_string(
            memory_extensions_root(&memory_root).join("ad_hoc/instructions.md")
        )
        .await
        .expect("read concurrently seeded instructions"),
        INSTRUCTIONS
    );
}

#[tokio::test]
async fn upgrades_legacy_managed_instructions() {
    let codex_home = TempDir::new().expect("create temp codex home");
    let memory_root = codex_home.path().join("memories");
    let instructions_path = memory_extensions_root(&memory_root).join("ad_hoc/instructions.md");
    tokio::fs::create_dir_all(instructions_path.parent().expect("instructions parent"))
        .await
        .expect("create extension directory");
    tokio::fs::write(&instructions_path, LEGACY_INSTRUCTIONS)
        .await
        .expect("write legacy instructions");

    seed_instructions(&memory_root)
        .await
        .expect("upgrade ad-hoc instructions");

    assert_eq!(
        tokio::fs::read_to_string(&instructions_path)
            .await
            .expect("read upgraded instructions"),
        INSTRUCTIONS
    );
}
