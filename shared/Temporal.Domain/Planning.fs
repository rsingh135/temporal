module Temporal.Domain.Planning

open Temporal.Domain.Types

/// Staging-preview logic: which nodes the user has toggled out of the
/// rehydration, and the payload the daemon receives on approval.

let isExcluded (nodeId: string) (excluded: string list) : bool =
    List.contains nodeId excluded

let toggleExcluded (nodeId: string) (excluded: string list) : string list =
    if isExcluded nodeId excluded then
        List.filter (fun id -> id <> nodeId) excluded
    else
        nodeId :: excluded

let includedNodes (w: WorkspaceState) (excluded: string list) : WindowNode list =
    w.Nodes |> List.filter (fun n -> not (isExcluded n.NodeId excluded))

/// Exclusions that don't match a node (stale UI state) are dropped.
let buildPayload (w: WorkspaceState) (excluded: string list) : RehydrationPayload =
    let valid =
        excluded
        |> List.filter (fun id -> w.Nodes |> List.exists (fun n -> n.NodeId = id))
    { Workspace = w; ExcludedNodeIds = valid }

/// Short human label for a node's captured state, shown in the staging list.
let nodeDetail (n: WindowNode) : string =
    match n.Payload with
    | BrowserWindow (tabs, _) ->
        let count = List.length tabs
        if count = 1 then "1 tab" else string count + " tabs"
    | TerminalWindow tabs ->
        match tabs with
        | [ t ] -> t.WorkingDirectory
        | ts -> string (List.length ts) + " terminal tabs"
    | EditorWindow (folderPath, _) -> folderPath
    | GenericWindow -> n.WindowTitle
