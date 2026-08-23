use super::*;

#[test]
fn validates_bounded_memory_mutations() {
    validate_note(
        "<memory_update version=\"1\"><operation>delete</operation><target>obsolete fact</target></memory_update>",
    )
    .expect("valid delete mutation");

    let oversized = format!(
        "<memory_update version=\"1\"><operation>add</operation><target>fact</target><content>{}</content></memory_update>",
        "x".repeat(AD_HOC_NOTE_MAX_BYTES)
    );
    let error = validate_note(&oversized).expect_err("oversized note should fail");
    assert!(error.to_string().contains("must be at most"));

    let control = "<memory_update version=\"1\"><operation>delete</operation><target>bad\0target</target></memory_update>";
    let error = validate_note(control).expect_err("control character should fail");
    assert!(error.to_string().contains("control characters"));
}

#[test]
fn reports_required_element_order() {
    let target_first = "<memory_update version=\"1\"><target>fact</target><operation>delete</operation></memory_update>";

    let error = validate_note(target_first).expect_err("out-of-order fields should fail");

    assert!(
        error
            .to_string()
            .contains("fields must appear as operation")
    );
}
