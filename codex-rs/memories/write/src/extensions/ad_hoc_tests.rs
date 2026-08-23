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

    assert_eq!(
        tokio::fs::read_to_string(&instructions_path)
            .await
            .expect("read seeded ad-hoc instructions"),
        INSTRUCTIONS
    );
    assert!(INSTRUCTIONS.contains("<memory_mutation_policy>"));
    assert!(INSTRUCTIONS.contains("Do not retain a"));
    assert!(INSTRUCTIONS.contains("tombstone"));

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
