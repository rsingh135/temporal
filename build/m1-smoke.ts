// M1 smoke: decode a canonical wire-format workspace through the Fable-generated
// TypeScript codec, re-encode it, and require byte-identical output.
// Must stay in sync with daemon/crates/temporald/src/main.rs.
import { workspaceFromWire, workspaceToWire } from "../ui/src/gen/domain/Codecs.ts";

const FIXTURE =
    '{"workspaceId":"ws-1","capturedAtUnixMs":1752300000000,"summary":"daemon work","tags":["rust","temporal"],"nodes":[{"nodeId":"n1","bundleId":"com.google.Chrome","appName":"Google Chrome","windowTitle":"Fable docs","geometry":{"x":0,"y":25,"width":1440,"height":875.5},"adapter":"chrome","payload":{"kind":"browser","tabs":[{"url":"https://fable.io","title":"Fable · docs"}],"activeTabIndex":0}}]}';

const decoded = workspaceFromWire(FIXTURE);
if (decoded.tag === 1) {
    console.error(`decode failed: ${decoded.fields[0]}`);
    process.exit(1);
}
const out = workspaceToWire(decoded.fields[0]);
if (out !== FIXTURE) {
    console.error(`roundtrip mismatch:\n  in:  ${FIXTURE}\n  out: ${out}`);
    process.exit(1);
}
console.log(`ui M1 smoke: codec roundtrip OK (${FIXTURE.length} chars)`);
