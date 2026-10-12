//! A-6 ④: malformed frontmatter never panics, never allocates unboundedly and is rejected
//! with an error, so a bad skill cannot break the settings screen; valid documents
//! round-trip.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use openside_skills::{
    parse, render, Document, Error, Frontmatter, Value, MAX_DESCRIPTION_CHARS, MAX_SKILL_MD_BYTES,
};
use proptest::prelude::*;

fn doc(name: &str, description: &str, body: &str) -> Document {
    Document {
        frontmatter: Frontmatter {
            name: name.into(),
            description: description.into(),
            license: None,
            compatibility: None,
            allowed_tools: Vec::new(),
            metadata: BTreeMap::new(),
            extra: BTreeMap::new(),
        },
        body: body.into(),
    }
}

#[test]
fn parses_the_common_agent_skills_shapes() {
    let text = "---\nname: pdf-tools\ndescription: Extract text from PDFs. Use when handling forms.\nlicense: MIT\nallowed-tools: Bash(pdftotext:*) Read\nmetadata:\n  author: someone\n  version: \"1.0\"\n---\n# Body\n\nSteps.\n";
    let document = parse(text).unwrap();
    assert_eq!(document.frontmatter.name, "pdf-tools");
    assert_eq!(
        document.frontmatter.description,
        "Extract text from PDFs. Use when handling forms."
    );
    assert_eq!(
        document.frontmatter.allowed_tools,
        ["Bash(pdftotext:*)", "Read"]
    );
    assert_eq!(document.frontmatter.metadata["version"], "1.0");
    assert_eq!(document.body, "# Body\n\nSteps.\n");
}

#[test]
fn block_scalars_flow_lists_and_unknown_keys_are_kept() {
    let text = "---\nname: notes\ndescription: >\n  First line\n  second line.\nallowed-tools: [Read, \"Bash(ls)\"]\ntags:\n  - a\n  - b\nargument-hint: '[file]'\n---\nbody";
    let document = parse(text).unwrap();
    assert_eq!(
        document.frontmatter.description,
        "First line second line.\n"
    );
    assert_eq!(document.frontmatter.allowed_tools, ["Read", "Bash(ls)"]);
    assert_eq!(
        document.frontmatter.extra["tags"],
        Value::List(vec!["a".into(), "b".into()])
    );
    assert_eq!(
        document.frontmatter.extra["argument-hint"],
        Value::Scalar("[file]".into())
    );
}

#[test]
fn crlf_and_bom_documents_parse_like_lf() {
    let text =
        "\u{feff}---\r\nname: crlf\r\ndescription: Works on windows files\r\n---\r\nbody\r\n";
    let document = parse(text).unwrap();
    assert_eq!(document.frontmatter.name, "crlf");
    assert_eq!(document.body, "body\n");
}

#[test]
fn rejects_documents_that_would_break_a_listing() {
    for (text, expect) in [
        ("no frontmatter", "frontmatter"),
        ("---\nname: a\ndescription: b\n", "not closed"),
        ("---\ndescription: only\n---\n", "name"),
        ("---\nname: a\n---\n", "description"),
        ("---\nname: Bad_Name\ndescription: x\n---\n", "name"),
        ("---\nname: -lead\ndescription: x\n---\n", "name"),
        ("---\nname: a--b\ndescription: x\n---\n", "name"),
        ("---\nname: a\nname: b\ndescription: x\n---\n", "duplicate"),
        ("---\nname: a\ndescription: x: y\n---\n", "quoted"),
        ("---\nname: a\ndescription: &anchor x\n---\n", "unsupported"),
        ("---\nname: a\ndescription: *alias\n---\n", "unsupported"),
        ("---\nname: a\ndescription: !!str x\n---\n", "unsupported"),
        ("---\nname: a\ndescription: \"open\n---\n", "unterminated"),
        ("---\nname: a\ndescription: x\n\tbad: 1\n---\n", "tabs"),
        (
            "---\nname: a\ndescription: \"x\"\n  stray: 1\n---\n",
            "indentation",
        ),
        (
            "---\nname: a\ndescription: x\nlicense: {a: 1}\n---\n",
            "flow mappings",
        ),
        (
            "---\nname: a\ndescription: x\nmetadata: nope\n---\n",
            "metadata",
        ),
    ] {
        let error = parse(text).expect_err(text).to_string();
        assert!(error.contains(expect), "`{text}` -> {error}");
    }
}

#[test]
fn enforces_size_limits() {
    let long = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
    let error = parse(&format!("---\nname: a\ndescription: {long}\n---\n")).unwrap_err();
    assert!(matches!(error, Error::Invalid { .. }), "{error}");
    let huge = format!(
        "---\nname: a\ndescription: x\n---\n{}",
        "y".repeat(MAX_SKILL_MD_BYTES)
    );
    assert!(matches!(parse(&huge).unwrap_err(), Error::TooLarge { .. }));
    let endless = format!("---\nname: a\n{}", "k: v\n".repeat(10_000));
    assert!(parse(&endless).is_err());
    let nul = "---\nname: a\ndescription: x\n---\nb\0";
    assert!(parse(nul).is_err());
}

#[test]
fn control_characters_in_values_are_rejected() {
    let error = parse("---\nname: a\ndescription: \"bell\\u0007\"\n---\n").unwrap_err();
    assert!(error.to_string().contains("control character"), "{error}");
}

#[test]
fn render_round_trips_awkward_values() {
    let mut expected = doc(
        "round-trip",
        "Says \"hi\": a # b, c\nsecond line\\end",
        "# Title\n\n---\nnot a delimiter inside the body\n",
    );
    expected.frontmatter.license = Some("true".into());
    expected.frontmatter.compatibility = Some("  padded ".into());
    expected.frontmatter.allowed_tools = vec!["Read".into(), "Bash(git:*)".into()];
    expected
        .frontmatter
        .metadata
        .insert("version".into(), "1.0".into());
    expected
        .frontmatter
        .metadata
        .insert("empty".into(), String::new());
    expected
        .frontmatter
        .extra
        .insert("tags".into(), Value::List(vec!["a b".into(), "-".into()]));
    expected
        .frontmatter
        .extra
        .insert("hint".into(), Value::Scalar("[file]".into()));
    expected
        .frontmatter
        .extra
        .insert("empty-list".into(), Value::List(Vec::new()));
    let text = render(&expected).unwrap();
    assert_eq!(parse(&text).unwrap(), expected, "{text}");
}

fn text_strategy() -> impl Strategy<Value = String> {
    // Printable text plus the characters that need quoting; no control characters.
    proptest::string::string_regex(
        "[ -~\u{a0}-\u{2ff}\u{3040}-\u{30ff}\u{1f300}-\u{1f64f}\n\t]{0,60}",
    )
    .unwrap()
}

fn name_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z0-9]{1,8}(-[a-z0-9]{1,8}){0,3}").unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic_and_stay_bounded(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = parse(&text);
        let _ = parse(&format!("---\n{text}\n---\nbody"));
    }

    #[test]
    fn structured_garbage_never_panics(lines in proptest::collection::vec("[ \\-:\\[\\]{}\"'|>&*!#a-z0-9,\\\\]{0,40}", 0..40)) {
        let text = format!("---\nname: fuzz\ndescription: d\n{}\n---\nbody", lines.join("\n"));
        if let Ok(document) = parse(&text) {
            // Whatever was accepted must also be renderable and stable.
            let rendered = render(&document).unwrap();
            prop_assert_eq!(parse(&rendered).unwrap(), document);
        }
    }

    #[test]
    fn valid_documents_round_trip(
        name in name_strategy(),
        description in text_strategy().prop_filter("non-blank", |s| !s.trim().is_empty()),
        body in text_strategy(),
        license in proptest::option::of(text_strategy().prop_filter("non-empty", |s| !s.is_empty())),
        tools in proptest::collection::vec(text_strategy().prop_filter("non-blank", |s| !s.trim().is_empty()), 0..4),
        metadata in proptest::collection::btree_map("[a-z][a-z0-9_]{0,10}", text_strategy(), 0..4),
    ) {
        let mut expected = doc(&name, &description, &body);
        expected.frontmatter.license = license;
        expected.frontmatter.allowed_tools = tools;
        expected.frontmatter.metadata = metadata;
        let text = render(&expected).unwrap();
        prop_assert_eq!(parse(&text).unwrap(), expected, "{}", text);
    }
}
