import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { IpcRequest } from "./gen/IpcRequest";
import type { IpcResponse } from "./gen/IpcResponse";
import type { QueryCandidate } from "./gen/QueryCandidate";
import type { WindowNode } from "./gen/WindowNode";
import type { AdapterKind } from "./gen/AdapterKind";
import { relativeAge, scorePercent } from "./format";

// ---------------------------------------------------------------------------
// Daemon plumbing (the Tauri shell ferries opaque JSON frames)
// ---------------------------------------------------------------------------

async function sendRequest(request: IpcRequest): Promise<IpcResponse[]> {
    const frames = await invoke<string[]>("daemon_request", {
        requestJson: JSON.stringify(request),
    });
    return frames.map((frame) => JSON.parse(frame) as IpcResponse);
}

function hidePanel(): void {
    void invoke("hide_panel");
}

// ---------------------------------------------------------------------------
// View helpers
// ---------------------------------------------------------------------------

const ADAPTER_GLYPHS: Record<AdapterKind, string> = {
    chrome: "🌐",
    "terminal-app": "🖥",
    vscode: "📝",
    cursor: "📝",
    generic: "▫️",
};

/** Short human label for a node's captured state, shown in the staging list. */
function nodeDetail(node: WindowNode): string {
    const p = node.payload;
    switch (p.kind) {
        case "browser":
            return p.tabs.length === 1 ? "1 tab" : `${p.tabs.length} tabs`;
        case "terminal":
            return p.tabs.length === 1 ? p.tabs[0].cwd : `${p.tabs.length} terminal tabs`;
        case "editor":
            return p.folderPath;
        case "generic":
            return node.windowTitle;
    }
}

type Phase =
    | { kind: "searching" }
    | { kind: "staging"; candidate: QueryCandidate }
    | { kind: "busy"; label: string }
    | { kind: "feedback"; ok: boolean; message: string };

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

function CandidateRow(props: {
    candidate: QueryCandidate;
    selected: boolean;
    onPick: () => void;
}) {
    const w = props.candidate.workspace;
    return (
        <div
            className={props.selected ? "candidate selected" : "candidate"}
            onClick={props.onPick}
        >
            <div className="candidate-main">
                <span className="candidate-summary">{w.summary}</span>
                <span className="candidate-meta">
                    {relativeAge(Date.now(), w.capturedAtUnixMs)} · {w.nodes.length} windows ·{" "}
                    {scorePercent(props.candidate.score)}
                </span>
            </div>
            <div className="candidate-tags">{w.tags.slice(0, 6).join("  ")}</div>
        </div>
    );
}

function NodeRow(props: { node: WindowNode; excluded: boolean; onToggle: () => void }) {
    return (
        <label className={props.excluded ? "node excluded" : "node"}>
            <input type="checkbox" checked={!props.excluded} onChange={props.onToggle} />
            <span className="node-glyph">{ADAPTER_GLYPHS[props.node.adapter]}</span>
            <div className="node-text">
                <span className="node-title">
                    {props.node.appName} — {props.node.windowTitle}
                </span>
                <span className="node-detail">{nodeDetail(props.node)}</span>
            </div>
        </label>
    );
}

export function App() {
    const [query, setQuery] = useState("");
    const [candidates, setCandidates] = useState<QueryCandidate[]>([]);
    const [phase, setPhase] = useState<Phase>({ kind: "searching" });
    const [excluded, setExcluded] = useState<Set<string>>(new Set());
    const [selectedIndex, setSelectedIndex] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);

    const runQuery = (text: string) => {
        sendRequest({ type: "query", text, limit: 8 })
            .then((responses) => {
                const last = responses.at(-1);
                if (last?.type === "query-results") {
                    setCandidates(last.candidates);
                    setSelectedIndex(0);
                } else if (last?.type === "error") {
                    setPhase({ kind: "feedback", ok: false, message: last.message });
                }
            })
            .catch((e) => setPhase({ kind: "feedback", ok: false, message: String(e) }));
    };

    // Debounced search-as-you-type.
    useEffect(() => {
        const id = setTimeout(() => runQuery(query), 200);
        return () => clearTimeout(id);
    }, [query]);

    // Panel re-shown via hotkey: reset to a fresh search.
    useEffect(() => {
        const unlisten = listen("panel-shown", () => {
            setPhase({ kind: "searching" });
            setExcluded(new Set());
            setQuery("");
            runQuery("");
            inputRef.current?.focus();
        });
        return () => {
            void unlisten.then((f) => f());
        };
    }, []);

    const finish = (responses: IpcResponse[]) => {
        const last = responses.at(-1);
        if (last?.type === "done") {
            setPhase({ kind: "feedback", ok: true, message: last.message });
        } else if (last?.type === "error") {
            setPhase({ kind: "feedback", ok: false, message: last.message });
        } else {
            setPhase({ kind: "feedback", ok: false, message: "unexpected daemon response" });
        }
    };

    const freeze = () => {
        setPhase({ kind: "busy", label: "Freezing current desktop…" });
        sendRequest({ type: "freeze" })
            .then(finish)
            .catch((e) => setPhase({ kind: "feedback", ok: false, message: String(e) }));
    };

    const rehydrate = (candidate: QueryCandidate) => {
        setPhase({ kind: "busy", label: "Rehydrating…" });
        // Only exclusions that still refer to real nodes go to the daemon.
        const valid = candidate.workspace.nodes
            .map((n) => n.nodeId)
            .filter((id) => excluded.has(id));
        sendRequest({
            type: "rehydrate",
            payload: { workspace: candidate.workspace, excludedNodeIds: valid },
        })
            .then(finish)
            .catch((e) => setPhase({ kind: "feedback", ok: false, message: String(e) }));
    };

    const stage = (candidate: QueryCandidate) => {
        setExcluded(new Set());
        setPhase({ kind: "staging", candidate });
    };

    const toggleExcluded = (nodeId: string) => {
        setExcluded((prev) => {
            const next = new Set(prev);
            if (next.has(nodeId)) next.delete(nodeId);
            else next.add(nodeId);
            return next;
        });
    };

    const onSearchKey = (key: string) => {
        if (key === "Escape") hidePanel();
        else if (key === "ArrowDown") {
            setSelectedIndex((i) => Math.min(i + 1, candidates.length - 1));
        } else if (key === "ArrowUp") {
            setSelectedIndex((i) => Math.max(i - 1, 0));
        } else if (key === "Enter") {
            const candidate = candidates[selectedIndex];
            if (candidate) stage(candidate);
        }
    };

    return (
        <div className="panel">
            {phase.kind === "searching" && (
                <>
                    <input
                        className="omnibar"
                        ref={inputRef}
                        autoFocus
                        placeholder="Summon a workspace…  (⌥Space to dismiss)"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={(e) => onSearchKey(e.key)}
                    />
                    <div className="candidates">
                        {candidates.map((candidate, i) => (
                            <CandidateRow
                                key={candidate.workspace.workspaceId}
                                candidate={candidate}
                                selected={i === selectedIndex}
                                onPick={() => stage(candidate)}
                            />
                        ))}
                    </div>
                    <div className="footer">
                        <span className="hint">↑↓ select · ⏎ stage · esc dismiss</span>
                        <button className="freeze" onClick={freeze}>
                            ❄ Freeze now
                        </button>
                    </div>
                </>
            )}
            {phase.kind === "staging" && (
                <>
                    <div className="staging-header">
                        <span className="staging-summary">{phase.candidate.workspace.summary}</span>
                        <span className="staging-meta">
                            {relativeAge(Date.now(), phase.candidate.workspace.capturedAtUnixMs)}
                        </span>
                    </div>
                    <div className="nodes">
                        {phase.candidate.workspace.nodes.map((node) => (
                            <NodeRow
                                key={node.nodeId}
                                node={node}
                                excluded={excluded.has(node.nodeId)}
                                onToggle={() => toggleExcluded(node.nodeId)}
                            />
                        ))}
                    </div>
                    <div className="footer">
                        <button className="back" onClick={() => setPhase({ kind: "searching" })}>
                            ← Back
                        </button>
                        <button className="approve" onClick={() => rehydrate(phase.candidate)}>
                            Rehydrate{" "}
                            {
                                phase.candidate.workspace.nodes.filter(
                                    (n) => !excluded.has(n.nodeId),
                                ).length
                            }{" "}
                            windows
                        </button>
                    </div>
                </>
            )}
            {phase.kind === "busy" && <div className="busy">{phase.label}</div>}
            {phase.kind === "feedback" && (
                <>
                    <div className={phase.ok ? "feedback ok" : "feedback err"}>{phase.message}</div>
                    <div className="footer">
                        <button className="back" onClick={() => setPhase({ kind: "searching" })}>
                            ← Back to search
                        </button>
                    </div>
                </>
            )}
        </div>
    );
}
