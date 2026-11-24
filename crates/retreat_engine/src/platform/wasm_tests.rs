//! `wasm-bindgen-test` tests: the browser-only logic of `wasm.rs` that `run_in_browser.mjs` does not
//! cover directly (URL parsing through the browser's own `URLSearchParams`, the `localStorage` save
//! backend, the snapshot memory, the status JSON the page reads). They run in headless Chrome:
//! `scripts/web/test_wasm.sh`.

use retreat_engine_api::SaveBackend;
use wasm_bindgen_test::*;

use super::*;
use crate::NetLaunch;

wasm_bindgen_test_configure!(run_in_browser);

fn pairs(query: &str) -> Vec<(String, String)> {
    url_query_pairs(query).expect("the query parses")
}

#[wasm_bindgen_test]
fn url_query_pairs_decode_the_way_the_browser_does() {
    assert_eq!(pairs(""), vec![]);
    assert_eq!(
        pairs("?commands=scripts%2Fa%20b.cmds&seed=3&x=a+b"),
        vec![
            ("commands".to_string(), "scripts/a b.cmds".to_string()),
            ("seed".to_string(), "3".to_string()),
            ("x".to_string(), "a b".to_string()),
        ]
    );
}

#[wasm_bindgen_test]
fn a_page_url_becomes_launch_options() {
    let options = LaunchOptions::from_url_query_pairs(pairs(
        "?commands=scripts/verify/a.cmds&seed=7&connect=https%3A%2F%2F127.0.0.1%3A4433&cert_hash=ab12&real_time=1",
    ))
    .expect("valid query");
    assert_eq!(options.seed, 7);
    assert!(options.real_time);
    assert_eq!(
        options.net,
        NetLaunch::Client {
            url: "https://127.0.0.1:4433".to_string(),
            cert_hash: Some("ab12".to_string()),
        }
    );
    let error = LaunchOptions::from_url_query_pairs(pairs("?headless=1")).unwrap_err();
    assert!(
        error.contains("unknown URL launch option `headless`"),
        "{error}"
    );
}

#[wasm_bindgen_test]
fn the_test_page_has_no_launch_options() {
    // The test runner's page URL has no query, so the real page-reading path gives the defaults.
    assert_eq!(page_launch_options(), Ok(LaunchOptions::default()));
}

#[wasm_bindgen_test]
fn the_status_json_matches_what_the_page_reads() {
    set_status(script_status_json(Some(&ScriptProgress::Running {
        last_line: 4,
    })));
    assert_eq!(retreat_status(), r#"{"script":"running","line":4}"#);
    set_status(script_status_json(Some(&ScriptProgress::Failed(
        "a \"quoted\" failure".to_string(),
    ))));
    assert_eq!(
        retreat_status(),
        r#"{"script":"failed","failure":"a \"quoted\" failure"}"#
    );
}

#[wasm_bindgen_test]
fn snapshots_are_kept_in_memory_for_the_page() {
    let location = store_snapshot("wasm_test_snapshot", vec![1, 2, 3]).expect("stored");
    assert!(location.contains("wasm_test_snapshot"), "{location}");
    assert!(
        retreat_snapshot_names()
            .iter()
            .any(|name| name.as_string().as_deref() == Some("wasm_test_snapshot"))
    );
    let png = retreat_snapshot_png("wasm_test_snapshot".to_string()).expect("present");
    assert_eq!(png.to_vec(), vec![1, 2, 3]);
    assert!(retreat_snapshot_png("wasm_test_missing".to_string()).is_none());
}

#[wasm_bindgen_test]
fn local_storage_backend_round_trips_and_lists_only_its_keys() {
    let mut backend = LocalStorageSaveBackend;
    // A foreign key must never show up in `keys` (it does not start with the save prefix).
    local_storage()
        .expect("localStorage exists in the test browser")
        .set_item("not.a.save", "x")
        .expect("set");
    backend.write("wasm_test_a", "{\"n\":1}").expect("write");
    assert_eq!(
        backend.read("wasm_test_a").expect("read"),
        Some("{\"n\":1}".to_string())
    );
    assert!(
        backend
            .keys()
            .expect("keys")
            .contains(&"wasm_test_a".to_string())
    );
    assert!(
        !backend
            .keys()
            .expect("keys")
            .iter()
            .any(|key| key.contains("not.a.save"))
    );
    assert!(backend.remove("wasm_test_a").expect("remove"));
    assert!(!backend.remove("wasm_test_a").expect("remove again"));
    assert_eq!(backend.read("wasm_test_a").expect("read"), None);
}
