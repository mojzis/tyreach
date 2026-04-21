//! Smoke test: parse `tiny_app/main.py`, extract call sites inside `main`.

use std::path::PathBuf;

use tyreach::extract::extract_call_sites;
use tyreach::parse::{parse_bytes, parse_file, ParsedFile};

#[test]
fn main_function_call_sites_include_foo_and_get() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main_path = manifest_dir.join("tests/fixtures/tiny_app/tiny_app/main.py");
    let parsed = parse_file(&main_path).expect("parse");

    // Locate the `main` function's byte range by walking top-level children.
    let root = parsed.tree.root_node();
    let mut cursor = root.walk();
    let main_fn = root
        .children(&mut cursor)
        .find(|n| {
            n.kind() == "function_definition"
                && n.child_by_field_name("name")
                    .and_then(|name| name.utf8_text(&parsed.source).ok())
                    == Some("main")
        })
        .expect("main function_definition");

    let sites = extract_call_sites(&parsed, main_fn.byte_range());
    assert!(!sites.is_empty(), "expected at least one call site in main");

    let callees: Vec<_> = sites.iter().map(|s| s.callee_text.as_str()).collect();
    assert!(callees.contains(&"foo"), "callees must include foo, got {callees:?}");
    assert!(
        callees.contains(&"get"),
        "callees must include get (for os.environ.get), got {callees:?}"
    );

    let source_lines = std::fs::read_to_string(&main_path).expect("read main.py").lines().count();
    for site in &sites {
        assert!(site.line > 0, "call site line should be > 0, got {}", site.line);
        assert!(
            (site.line as usize) < source_lines,
            "call site line {} should be within file ({} lines)",
            site.line,
            source_lines
        );
    }
}

/// Regression: walker-side callers must scope `extract_call_sites` to the
/// function body, so Typer-style parameter defaults like `typer.Option(...)`
/// are not picked up as outgoing call sites. This test feeds the body-only
/// byte range (mirroring the walker) and asserts `Option` is absent while the
/// real body call `print` is present.
#[test]
fn parameter_default_calls_are_not_extracted_from_body() {
    let source = br#"import typer

def cmd(
    x: str = typer.Option("", help="..."),
    y: int = typer.Argument(0),
):
    """Body."""
    print(x)
"#
    .to_vec();
    let parsed: ParsedFile = parse_bytes(source, PathBuf::from("fixture.py")).expect("parse");

    let root = parsed.tree.root_node();
    let mut cursor = root.walk();
    let fn_node = root
        .children(&mut cursor)
        .find(|n| {
            n.kind() == "function_definition"
                && n.child_by_field_name("name")
                    .and_then(|name| name.utf8_text(&parsed.source).ok())
                    == Some("cmd")
        })
        .expect("cmd function_definition");

    let body = fn_node.child_by_field_name("body").expect("function has a body");
    let sites = extract_call_sites(&parsed, body.byte_range());

    let callees: Vec<_> = sites.iter().map(|s| s.callee_text.as_str()).collect();
    assert!(callees.contains(&"print"), "body scope should include print, got {callees:?}");
    assert!(
        !callees.contains(&"Option"),
        "parameter-default typer.Option must NOT be extracted, got {callees:?}"
    );
    assert!(
        !callees.contains(&"Argument"),
        "parameter-default typer.Argument must NOT be extracted, got {callees:?}"
    );
}
