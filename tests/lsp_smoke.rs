//! Smoke test: spawn ty LSP server against `tiny_app`, resolve `foo(` call
//! site to `lib.py` via `goto_definition`. Skips cleanly when ty isn't
//! available.

use std::path::PathBuf;

use tyreach::lsp::client::TyLspClient;

#[tokio::test(flavor = "multi_thread")]
async fn goto_definition_resolves_foo_to_lib() {
    if std::env::var("TY_AVAILABLE").is_err() {
        eprintln!("skipping: set TY_AVAILABLE=1 to run");
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest_dir.join("tests/fixtures/tiny_app");
    let workspace = fixture
        .canonicalize()
        .unwrap_or_else(|e| panic!("canonicalize fixture: {e}"))
        .to_string_lossy()
        .into_owned();

    let client = TyLspClient::new(&workspace).await.expect("start TyLspClient");

    let main_path = fixture.join("tiny_app/main.py");
    let main_str = main_path.to_string_lossy().into_owned();
    client.open_document(&main_str).await.expect("didOpen");

    // main.py:
    //   0: import os
    //   1:
    //   2: from tiny_app.lib import foo
    //   3:
    //   4:
    //   5: def main() -> str:
    //   6:     value = foo()
    // `foo(` at line 6, the first column of "foo" is character 12.
    let src = std::fs::read_to_string(&main_path).expect("read main.py");
    let line_idx = src
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("value = foo()"))
        .map(|(i, _)| i)
        .expect("find foo() call");
    let line = src.lines().nth(line_idx).expect("line");
    let col = line.find("foo()").expect("locate foo(");

    let line_u32 = u32::try_from(line_idx).expect("line fits in u32");
    let col_u32 = u32::try_from(col).expect("column fits in u32");
    let locations =
        client.goto_definition(&main_str, line_u32, col_u32).await.expect("goto_definition call");

    assert!(!locations.is_empty(), "ty returned no locations for foo call");
    let first = &locations[0];
    assert!(first.uri.ends_with("lib.py"), "expected definition in lib.py, got uri={}", first.uri);
}
