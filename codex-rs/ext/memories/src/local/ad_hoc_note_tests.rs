use super::*;

#[test]
fn validates_bounded_memory_mutations() {
    validate_note(
        "<memory_update version=\"1\"><operation>delete</operation><target>obsolete fact</target></memory_update>",
    )
    .expect("valid delete mutation");
    validate_note(
        "<memory_update version=\"1\"><operation>update</operation><target>AT&amp;T fact</target><content>replacement</content></memory_update>",
    )
    .expect("valid update mutation");

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
fn rejects_invalid_memory_mutation_shapes() {
    let cases = [
        (
            "<memory_update version=\"1\"><target>fact</target><operation>delete</operation></memory_update>",
            "fields must appear as operation",
        ),
        (
            "<memory_update version=\"1\"><operation>remove</operation><target>fact</target></memory_update>",
            "operation must be add, update, or delete",
        ),
        (
            "<memory_update version=\"1\"><operation>delete</operation><target> </target></memory_update>",
            "target must not be empty",
        ),
        (
            "<memory_update version=\"1\"><operation>add</operation><target>fact</target></memory_update>",
            "require non-empty content",
        ),
        (
            "<memory_update version=\"1\"><operation>update</operation><target>fact</target><content> </content></memory_update>",
            "require non-empty content",
        ),
        (
            "<memory_update version=\"1\"><operation>delete</operation><target>fact</target><content>extra</content></memory_update>",
            "must omit content",
        ),
        (
            "<memory_update version=\"1\"><operation>delete</operation><target>bad<target</target></memory_update>",
            "valid XML escaping",
        ),
        (
            "<memory_update version=\"1\"><operation>delete</operation><target>AT&T</target></memory_update>",
            "valid XML escaping",
        ),
        (
            "<memory_update version=\"1\"><operation>add</operation><target>fact</target><content >value</content></memory_update>",
            "unexpected content",
        ),
        (
            "<memory_update version=\"1\"><operation>delete</operation><target>fact</target>junk</memory_update>",
            "unexpected content",
        ),
    ];

    for (note, expected) in cases {
        let error = validate_note(note).expect_err("invalid mutation should fail");
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?} in {error}"
        );
    }
}
