module Temporal.Domain.Tagging

open Temporal.Domain.Json
open Temporal.Domain.Types

/// Heuristic auto-tagging: deterministic tags and a compact summary derived
/// purely from extracted state. This is the floor the semantic index always
/// has; LLM-generated tags (daemon-side) enrich on top of it asynchronously.

let private urlHost (url: string) : string =
    let idx = url.IndexOf("://")
    if idx < 0 then
        ""
    else
        let rest = url.Substring(idx + 3)
        let slash = rest.IndexOf("/")
        let host = if slash < 0 then rest else rest.Substring(0, slash)
        let colon = host.IndexOf(":")
        let host = if colon < 0 then host else host.Substring(0, colon)
        if host.StartsWith("www.") then host.Substring(4) else host

let private baseName (path: string) : string =
    let trimmed =
        if path.EndsWith("/") && path.Length > 1 then path.Substring(0, path.Length - 1)
        else path
    let idx = trimmed.LastIndexOf("/")
    if idx < 0 then trimmed else trimmed.Substring(idx + 1)

let private appendDistinct (seen: string list) (x: string) : string list =
    if x = "" || List.contains x seen then seen else seen @ [ x ]

let private distinct (xs: string list) : string list =
    List.fold appendDistinct [] xs

/// Tag sources, in priority order: project folders (editors, terminals),
/// URL hosts, app names.
let deriveTags (w: WorkspaceState) : string list =
    let projectTags =
        w.Nodes
        |> List.collect (fun n ->
            match n.Payload with
            | EditorWindow (folderPath, _) -> [ baseName folderPath ]
            | TerminalWindow tabs -> tabs |> List.map (fun t -> baseName t.WorkingDirectory)
            | _ -> [])
    let hostTags =
        w.Nodes
        |> List.collect (fun n ->
            match n.Payload with
            | BrowserWindow (tabs, _) -> tabs |> List.map (fun t -> urlHost t.Url)
            | _ -> [])
    let appTags = w.Nodes |> List.map (fun n -> n.AppName.ToLower())
    let all = List.map (fun (t: string) -> t.ToLower()) (projectTags @ hostTags) @ appTags
    distinct all |> List.truncate 24

/// One line the staging UI can show before LLM tags arrive, e.g.
/// "temporal · remy-ios · 23 Chrome tabs · Finder · Notes"
let deriveSummary (w: WorkspaceState) : string =
    let projects =
        w.Nodes
        |> List.collect (fun n ->
            match n.Payload with
            | EditorWindow (folderPath, _) -> [ baseName folderPath ]
            | TerminalWindow tabs ->
                match tabs with
                | first :: _ -> [ baseName first.WorkingDirectory ]
                | [] -> []
            | _ -> [])
        |> distinct
    let tabCount =
        w.Nodes
        |> List.sumBy (fun n ->
            match n.Payload with
            | BrowserWindow (tabs, _) -> List.length tabs
            | _ -> 0)
    let browserPart =
        if tabCount = 0 then []
        elif tabCount = 1 then [ "1 browser tab" ]
        else [ string tabCount + " browser tabs" ]
    let genericApps =
        w.Nodes
        |> List.collect (fun n ->
            match n.Payload with
            | GenericWindow -> [ n.AppName ]
            | _ -> [])
        |> distinct
        |> List.truncate 4
    let parts = projects @ browserPart @ genericApps
    if List.isEmpty parts then "empty workspace" else String.concat " · " parts

/// Fills Tags and Summary from the captured nodes.
let enrich (w: WorkspaceState) : WorkspaceState =
    { w with Tags = deriveTags w; Summary = deriveSummary w }

/// The Tags list as canonical wire JSON (for the denormalized DB column).
let tagsToWire (w: WorkspaceState) : string =
    print (JArray (List.map JString w.Tags))
