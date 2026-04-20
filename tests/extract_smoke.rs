//! Smoke test: parse `tiny_app/main.py`, extract call sites inside `main`.

use std::path::PathBuf;

use tyreach::extract::extract_call_sites;
use tyreach::parse::parse_file;

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
