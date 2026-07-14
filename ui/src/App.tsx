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

function permissionBannerText(p: { screenRecording: boolean; accessibility: boolean }): string | null {
    const missing: string[] = [];
    if (!p.screenRecording) missing.push("Screen Recording");
    if (!p.accessibility) missing.push("Accessibility");
    if (missing.length === 0) return null;
    return `${missing.join(" & ")} permission needed — enable in System Settings → Privacy & Security.`;
}

type NodeStatus = {
    nodeId: string;
    appName: string;
    state: "pending" | "ok" | "err";
    message?: string;
};

type Phase =
    | { kind: "searching" }
    | { kind: "staging"; candidate: QueryCandidate }
    | { kind: "busy"; label: string; nodes?: NodeStatus[]; percent?: number }
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
    const kind = props.candidate.kind;
    const classes = ["candidate", kind !== "workspace" && kind, props.selected && "selected"]
        .filter(Boolean)
        .join(" ");
    return (
        <div className={classes} onClick={props.onPick}>
            <div className="candidate-main">
                <span className="candidate-summary">
                    {kind === "group" && <span className="candidate-glyph">↳</span>}
                    {kind === "assembled" && <span className="candidate-glyph">✦</span>}
                    {w.summary}
                </span>
                <span className="candidate-meta">
                    {relativeAge(Date.now(), w.capturedAtUnixMs)} · {w.nodes.length} windows ·{" "}
                    {scorePercent(props.candidate.score)}
                </span>
            </div>
            <div className="candidate-tags">{w.tags.slice(0, 6).join("  ")}</div>
        </div>
    );
}

function NodeRow(props: {
    node: WindowNode;
    excluded: boolean;
    focused: boolean;
    onToggle: () => void;
}) {
    const classes = ["node", props.excluded && "excluded", props.focused && "focused"]
        .filter(Boolean)
        .join(" ");
    return (
        <label className={classes}>
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

function BusyNodeRow(props: { status: NodeStatus }) {
    const icon = props.status.state === "pending" ? "⏳" : props.status.state === "ok" ? "✓" : "✕";
    return (
        <div className={`busy-node ${props.status.state}`}>
            <span className="busy-node-icon">{icon}</span>
            <span className="busy-node-name">{props.status.appName}</span>
            {props.status.message && <span className="busy-node-message">{props.status.message}</span>}
        </div>
    );
}

export function App() {
    const [query, setQuery] = useState("");
    const [candidates, setCandidates] = useState<QueryCandidate[]>([]);
    const [searching, setSearching] = useState(false);
    const [phase, setPhase] = useState<Phase>({ kind: "searching" });
    const [excluded, setExcluded] = useState<Set<string>>(new Set());
    const [selectedIndex, setSelectedIndex] = useState(0);
    const [stagingIndex, setStagingIndex] = useState(0);
    const [permissions, setPermissions] = useState<{
        screenRecording: boolean;
        accessibility: boolean;
    } | null>(null);
    const inputRef = useRef<HTMLInputElement>(null);
    const nodesContainerRef = useRef<HTMLDivElement>(null);

    const checkPermissions = () => {
        sendRequest({ type: "permission-status" })
            .then((responses) => {
                const last = responses.at(-1);
                if (last?.type === "permission-status") {
                    setPermissions({
                        screenRecording: last.screenRecording,
                        accessibility: last.accessibility,
                    });
                }
            })
            .catch(() => {
                // Best-effort diagnostics; a failed check just leaves the banner hidden.
            });
    };

    const runQuery = (text: string) => {
        setSearching(true);
        sendRequest({ type: "query", text, limit: 8 })
            .then((responses) => {
                setSearching(false);
                const last = responses.at(-1);
                if (last?.type === "query-results") {
                    setCandidates(last.candidates);
                    setSelectedIndex(0);
                } else if (last?.type === "error") {
                    setPhase({ kind: "feedback", ok: false, message: last.message });
                }
            })
            .catch((e) => {
                setSearching(false);
                setPhase({ kind: "feedback", ok: false, message: String(e) });
            });
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
            checkPermissions();
            inputRef.current?.focus();
        });
        return () => {
            void unlisten.then((f) => f());
        };
    }, []);

    // Permission status at startup, before the panel is ever shown.
    useEffect(() => {
        checkPermissions();
    }, []);

    // Live per-node rehydration progress, streamed by the daemon as each node
    // starts/finishes; only applied while a "busy" phase is on screen.
    useEffect(() => {
        const unlisten = listen<string>("daemon-response", (event) => {
            let response: IpcResponse;
            try {
                response = JSON.parse(event.payload) as IpcResponse;
            } catch {
                return;
            }
            setPhase((prev) => {
                if (prev.kind !== "busy") return prev;
                if (response.type === "progress") {
                    return { ...prev, label: response.detail || response.stage, percent: response.percent };
                }
                if (response.type === "node-result" && prev.nodes) {
                    const nodes = prev.nodes.map((n) =>
                        n.nodeId === response.nodeId
                            ? {
                                  ...n,
                                  state: (response.ok ? "ok" : "err") as NodeStatus["state"],
                                  message: response.message ?? undefined,
                              }
                            : n,
                    );
                    return { ...prev, nodes };
                }
                return prev;
            });
        });
        return () => {
            void unlisten.then((f) => f());
        };
    }, []);

    // Autofocus the staging list so arrow keys work without a click first.
    useEffect(() => {
        if (phase.kind === "staging") {
            nodesContainerRef.current?.focus();
        }
    }, [phase.kind]);

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
        const included = candidate.workspace.nodes.filter((n) => !excluded.has(n.nodeId));
        setPhase({
            kind: "busy",
            label: "Rehydrating…",
            percent: 0,
            nodes: included.map((n) => ({ nodeId: n.nodeId, appName: n.appName, state: "pending" })),
        });
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
        setStagingIndex(0);
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

    const onStagingKey = (e: React.KeyboardEvent, nodes: WindowNode[]) => {
        if (e.key === "ArrowDown") {
            e.preventDefault();
            setStagingIndex((i) => Math.min(i + 1, nodes.length - 1));
        } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setStagingIndex((i) => Math.max(i - 1, 0));
        } else if (e.key === " " || e.key === "Enter") {
            e.preventDefault();
            const node = nodes[stagingIndex];
            if (node) toggleExcluded(node.nodeId);
        } else if (e.key === "Escape") {
            setPhase({ kind: "searching" });
        }
    };

    const permissionBanner = permissions ? permissionBannerText(permissions) : null;

    return (
        <div className="panel">
            {phase.kind === "searching" && (
                <>
                    {permissionBanner && <div className="permission-banner">{permissionBanner}</div>}
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
                        {!searching && candidates.length === 0 && (
                            <div className="empty-state">
                                {query.trim() === ""
                                    ? "Freeze your first workspace."
                                    : `No workspaces match "${query}".`}
                            </div>
                        )}
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
                        <span className="hint">
                            {searching ? "Searching…" : "↑↓ select · ⏎ stage · esc dismiss"}
                        </span>
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
                    <div
                        className="nodes"
                        tabIndex={0}
                        ref={nodesContainerRef}
                        onKeyDown={(e) => onStagingKey(e, phase.candidate.workspace.nodes)}
                    >
                        {phase.candidate.workspace.nodes.map((node, i) => (
                            <NodeRow
                                key={node.nodeId}
                                node={node}
                                excluded={excluded.has(node.nodeId)}
                                focused={i === stagingIndex}
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
            {phase.kind === "busy" && (
                <div className="busy">
                    <div className="busy-label">{phase.label}</div>
                    {phase.percent !== undefined && (
                        <div className="busy-bar">
                            <div className="busy-bar-fill" style={{ width: `${phase.percent}%` }} />
                        </div>
                    )}
                    {phase.nodes && (
                        <div className="busy-nodes">
                            {phase.nodes.map((status) => (
                                <BusyNodeRow key={status.nodeId} status={status} />
                            ))}
                        </div>
                    )}
                </div>
            )}
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
