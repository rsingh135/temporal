use fable_library_rust::String_::string;
use temporal_core::Temporal::Domain::Codecs::{workspaceFromWire, workspaceToWire};

// M1 smoke: decode a canonical wire-format workspace through the Fable-generated
// codec, re-encode it, and require byte-identical output.
const FIXTURE: &str = r#"{"workspaceId":"ws-1","capturedAtUnixMs":1752300000000,"summary":"daemon work","tags":["rust","temporal"],"nodes":[{"nodeId":"n1","bundleId":"com.google.Chrome","appName":"Google Chrome","windowTitle":"Fable docs","geometry":{"x":0,"y":25,"width":1440,"height":875.5},"adapter":"chrome","payload":{"kind":"browser","tabs":[{"url":"https://fable.io","title":"Fable · docs"}],"activeTabIndex":0}}]}"#;

fn main() {
    match workspaceFromWire(string(FIXTURE)) {
        Ok(workspace) => {
            let out = workspaceToWire(workspace);
            if out.to_string() == FIXTURE {
                println!("temporald M1 smoke: codec roundtrip OK ({} bytes)", FIXTURE.len());
            } else {
                eprintln!("roundtrip mismatch:\n  in:  {FIXTURE}\n  out: {out}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("decode failed: {e}");
            std::process::exit(1);
        }
    }
}
