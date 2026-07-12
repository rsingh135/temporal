module Temporal.Domain.Codecs

open Temporal.Domain.Json
open Temporal.Domain.Types

// ---------------------------------------------------------------------------
// Decode helpers
// ---------------------------------------------------------------------------

let private field (name: string) (fields: (string * JsonValue) list) : Result<JsonValue, string> =
    match List.tryFind (fun (n, _) -> n = name) fields with
    | Some (_, v) -> Ok v
    | None -> Error ("missing field '" + name + "'")

let private asObject (v: JsonValue) : Result<(string * JsonValue) list, string> =
    match v with
    | JObject fs -> Ok fs
    | _ -> Error "expected object"

let private asArray (v: JsonValue) : Result<JsonValue list, string> =
    match v with
    | JArray xs -> Ok xs
    | _ -> Error "expected array"

let private asString (v: JsonValue) : Result<string, string> =
    match v with
    | JString s -> Ok s
    | _ -> Error "expected string"

let private asNumber (v: JsonValue) : Result<float, string> =
    match v with
    | JNumber n -> Ok n
    | _ -> Error "expected number"

let private asInt (v: JsonValue) : Result<int, string> =
    match v with
    | JNumber n -> Ok (int n)
    | _ -> Error "expected number"

let private asInt64 (v: JsonValue) : Result<int64, string> =
    match v with
    | JNumber n -> Ok (int64 n)
    | _ -> Error "expected number"

let private strField (name: string) (fields: (string * JsonValue) list) : Result<string, string> =
    field name fields |> Result.bind asString

let private traverse (f: 'a -> Result<'b, string>) (xs: 'a list) : Result<'b list, string> =
    let folder (acc: Result<'b list, string>) (x: 'a) : Result<'b list, string> =
        match acc with
        | Error e -> Error e
        | Ok ys ->
            match f x with
            | Ok y -> Ok (y :: ys)
            | Error e -> Error e
    match List.fold folder (Ok []) xs with
    | Ok ys -> Ok (List.rev ys)
    | Error e -> Error e

// ---------------------------------------------------------------------------
// WindowGeometry
// ---------------------------------------------------------------------------

let encodeGeometry (g: WindowGeometry) : JsonValue =
    JObject
        [ "x", JNumber g.X
          "y", JNumber g.Y
          "width", JNumber g.Width
          "height", JNumber g.Height ]

let decodeGeometry (v: JsonValue) : Result<WindowGeometry, string> =
    asObject v
    |> Result.bind (fun fs ->
        field "x" fs |> Result.bind asNumber
        |> Result.bind (fun x ->
            field "y" fs |> Result.bind asNumber
            |> Result.bind (fun y ->
                field "width" fs |> Result.bind asNumber
                |> Result.bind (fun w ->
                    field "height" fs |> Result.bind asNumber
                    |> Result.map (fun h -> { X = x; Y = y; Width = w; Height = h })))))

// ---------------------------------------------------------------------------
// AdapterKind
// ---------------------------------------------------------------------------

let encodeAdapterKind (k: AdapterKind) : JsonValue =
    JString
        (match k with
         | Chrome -> "chrome"
         | TerminalApp -> "terminal-app"
         | VSCode -> "vscode"
         | Cursor -> "cursor"
         | Generic -> "generic")

let decodeAdapterKind (v: JsonValue) : Result<AdapterKind, string> =
    asString v
    |> Result.bind (fun s ->
        if s = "chrome" then Ok Chrome
        elif s = "terminal-app" then Ok TerminalApp
        elif s = "vscode" then Ok VSCode
        elif s = "cursor" then Ok Cursor
        elif s = "generic" then Ok Generic
        else Error ("unknown adapter kind '" + s + "'"))

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

let encodeBrowserTab (t: BrowserTab) : JsonValue =
    JObject [ "url", JString t.Url; "title", JString t.Title ]

let decodeBrowserTab (v: JsonValue) : Result<BrowserTab, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "url" fs
        |> Result.bind (fun url ->
            strField "title" fs
            |> Result.map (fun title -> { Url = url; Title = title })))

let encodeTerminalTab (t: TerminalTab) : JsonValue =
    JObject [ "tty", JString t.Tty; "cwd", JString t.WorkingDirectory ]

let decodeTerminalTab (v: JsonValue) : Result<TerminalTab, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "tty" fs
        |> Result.bind (fun tty ->
            strField "cwd" fs
            |> Result.map (fun cwd -> { Tty = tty; WorkingDirectory = cwd })))

// ---------------------------------------------------------------------------
// NodePayload
// ---------------------------------------------------------------------------

let encodeNodePayload (p: NodePayload) : JsonValue =
    match p with
    | BrowserWindow (tabs, activeTabIndex) ->
        JObject
            [ "kind", JString "browser"
              "tabs", JArray (List.map encodeBrowserTab tabs)
              "activeTabIndex", JNumber (float activeTabIndex) ]
    | TerminalWindow tabs ->
        JObject
            [ "kind", JString "terminal"
              "tabs", JArray (List.map encodeTerminalTab tabs) ]
    | EditorWindow (folderPath, openFiles) ->
        JObject
            [ "kind", JString "editor"
              "folderPath", JString folderPath
              "openFiles", JArray (List.map JString openFiles) ]
    | GenericWindow -> JObject [ "kind", JString "generic" ]

let decodeNodePayload (v: JsonValue) : Result<NodePayload, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "kind" fs
        |> Result.bind (fun kind ->
            if kind = "browser" then
                field "tabs" fs |> Result.bind asArray
                |> Result.bind (traverse decodeBrowserTab)
                |> Result.bind (fun tabs ->
                    field "activeTabIndex" fs |> Result.bind asInt
                    |> Result.map (fun active -> BrowserWindow (tabs, active)))
            elif kind = "terminal" then
                field "tabs" fs |> Result.bind asArray
                |> Result.bind (traverse decodeTerminalTab)
                |> Result.map TerminalWindow
            elif kind = "editor" then
                strField "folderPath" fs
                |> Result.bind (fun folderPath ->
                    field "openFiles" fs |> Result.bind asArray
                    |> Result.bind (traverse asString)
                    |> Result.map (fun openFiles -> EditorWindow (folderPath, openFiles)))
            elif kind = "generic" then Ok GenericWindow
            else Error ("unknown payload kind '" + kind + "'")))

// ---------------------------------------------------------------------------
// WindowNode / WorkspaceState
// ---------------------------------------------------------------------------

let encodeWindowNode (n: WindowNode) : JsonValue =
    JObject
        [ "nodeId", JString n.NodeId
          "bundleId", JString n.BundleId
          "appName", JString n.AppName
          "windowTitle", JString n.WindowTitle
          "geometry", encodeGeometry n.Geometry
          "adapter", encodeAdapterKind n.Adapter
          "payload", encodeNodePayload n.Payload ]

let decodeWindowNode (v: JsonValue) : Result<WindowNode, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "nodeId" fs
        |> Result.bind (fun nodeId ->
            strField "bundleId" fs
            |> Result.bind (fun bundleId ->
                strField "appName" fs
                |> Result.bind (fun appName ->
                    strField "windowTitle" fs
                    |> Result.bind (fun windowTitle ->
                        field "geometry" fs |> Result.bind decodeGeometry
                        |> Result.bind (fun geometry ->
                            field "adapter" fs |> Result.bind decodeAdapterKind
                            |> Result.bind (fun adapter ->
                                field "payload" fs |> Result.bind decodeNodePayload
                                |> Result.map (fun payload ->
                                    { NodeId = nodeId
                                      BundleId = bundleId
                                      AppName = appName
                                      WindowTitle = windowTitle
                                      Geometry = geometry
                                      Adapter = adapter
                                      Payload = payload }))))))))

let encodeWorkspaceState (w: WorkspaceState) : JsonValue =
    JObject
        [ "workspaceId", JString w.WorkspaceId
          "capturedAtUnixMs", JNumber (float w.CapturedAtUnixMs)
          "summary", JString w.Summary
          "tags", JArray (List.map JString w.Tags)
          "nodes", JArray (List.map encodeWindowNode w.Nodes) ]

let decodeWorkspaceState (v: JsonValue) : Result<WorkspaceState, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "workspaceId" fs
        |> Result.bind (fun workspaceId ->
            field "capturedAtUnixMs" fs |> Result.bind asInt64
            |> Result.bind (fun capturedAt ->
                strField "summary" fs
                |> Result.bind (fun summary ->
                    field "tags" fs |> Result.bind asArray
                    |> Result.bind (traverse asString)
                    |> Result.bind (fun tags ->
                        field "nodes" fs |> Result.bind asArray
                        |> Result.bind (traverse decodeWindowNode)
                        |> Result.map (fun nodes ->
                            { WorkspaceId = workspaceId
                              CapturedAtUnixMs = capturedAt
                              Summary = summary
                              Tags = tags
                              Nodes = nodes }))))))

// ---------------------------------------------------------------------------
// RehydrationPayload / QueryCandidate
// ---------------------------------------------------------------------------

let encodeRehydrationPayload (p: RehydrationPayload) : JsonValue =
    JObject
        [ "workspace", encodeWorkspaceState p.Workspace
          "excludedNodeIds", JArray (List.map JString p.ExcludedNodeIds) ]

let decodeRehydrationPayload (v: JsonValue) : Result<RehydrationPayload, string> =
    asObject v
    |> Result.bind (fun fs ->
        field "workspace" fs |> Result.bind decodeWorkspaceState
        |> Result.bind (fun workspace ->
            field "excludedNodeIds" fs |> Result.bind asArray
            |> Result.bind (traverse asString)
            |> Result.map (fun excluded ->
                { Workspace = workspace; ExcludedNodeIds = excluded })))

let encodeQueryCandidate (c: QueryCandidate) : JsonValue =
    JObject
        [ "workspace", encodeWorkspaceState c.Workspace
          "score", JNumber c.Score ]

let decodeQueryCandidate (v: JsonValue) : Result<QueryCandidate, string> =
    asObject v
    |> Result.bind (fun fs ->
        field "workspace" fs |> Result.bind decodeWorkspaceState
        |> Result.bind (fun workspace ->
            field "score" fs |> Result.bind asNumber
            |> Result.map (fun score -> { Workspace = workspace; Score = score })))

// ---------------------------------------------------------------------------
// IPC envelope
// ---------------------------------------------------------------------------

let encodeIpcRequest (r: IpcRequest) : JsonValue =
    match r with
    | Ping -> JObject [ "type", JString "ping" ]
    | Freeze -> JObject [ "type", JString "freeze" ]
    | Query (text, limit) ->
        JObject
            [ "type", JString "query"
              "text", JString text
              "limit", JNumber (float limit) ]
    | Rehydrate payload ->
        JObject
            [ "type", JString "rehydrate"
              "payload", encodeRehydrationPayload payload ]

let decodeIpcRequest (v: JsonValue) : Result<IpcRequest, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "type" fs
        |> Result.bind (fun t ->
            if t = "ping" then Ok Ping
            elif t = "freeze" then Ok Freeze
            elif t = "query" then
                strField "text" fs
                |> Result.bind (fun text ->
                    field "limit" fs |> Result.bind asInt
                    |> Result.map (fun limit -> Query (text, limit)))
            elif t = "rehydrate" then
                field "payload" fs
                |> Result.bind decodeRehydrationPayload
                |> Result.map Rehydrate
            else Error ("unknown request type '" + t + "'")))

let encodeIpcResponse (r: IpcResponse) : JsonValue =
    match r with
    | Pong -> JObject [ "type", JString "pong" ]
    | FreezeStarted workspaceId ->
        JObject
            [ "type", JString "freeze-started"
              "workspaceId", JString workspaceId ]
    | QueryResults candidates ->
        JObject
            [ "type", JString "query-results"
              "candidates", JArray (List.map encodeQueryCandidate candidates) ]
    | RehydrateStarted -> JObject [ "type", JString "rehydrate-started" ]
    | Progress (stage, detail, percent) ->
        JObject
            [ "type", JString "progress"
              "stage", JString stage
              "detail", JString detail
              "percent", JNumber (float percent) ]
    | Done message ->
        JObject [ "type", JString "done"; "message", JString message ]
    | IpcError (code, message) ->
        JObject
            [ "type", JString "error"
              "code", JString code
              "message", JString message ]

let decodeIpcResponse (v: JsonValue) : Result<IpcResponse, string> =
    asObject v
    |> Result.bind (fun fs ->
        strField "type" fs
        |> Result.bind (fun t ->
            if t = "pong" then Ok Pong
            elif t = "freeze-started" then
                strField "workspaceId" fs |> Result.map FreezeStarted
            elif t = "query-results" then
                field "candidates" fs |> Result.bind asArray
                |> Result.bind (traverse decodeQueryCandidate)
                |> Result.map QueryResults
            elif t = "rehydrate-started" then Ok RehydrateStarted
            elif t = "progress" then
                strField "stage" fs
                |> Result.bind (fun stage ->
                    strField "detail" fs
                    |> Result.bind (fun detail ->
                        field "percent" fs |> Result.bind asInt
                        |> Result.map (fun percent -> Progress (stage, detail, percent))))
            elif t = "done" then strField "message" fs |> Result.map Done
            elif t = "error" then
                strField "code" fs
                |> Result.bind (fun code ->
                    strField "message" fs
                    |> Result.map (fun message -> IpcError (code, message)))
            else Error ("unknown response type '" + t + "'")))

// ---------------------------------------------------------------------------
// Wire entry points (string <-> value)
// ---------------------------------------------------------------------------

let requestToWire (r: IpcRequest) : string = print (encodeIpcRequest r)

let requestFromWire (s: string) : Result<IpcRequest, string> =
    parse s |> Result.bind decodeIpcRequest

let responseToWire (r: IpcResponse) : string = print (encodeIpcResponse r)

let responseFromWire (s: string) : Result<IpcResponse, string> =
    parse s |> Result.bind decodeIpcResponse

let workspaceToWire (w: WorkspaceState) : string = print (encodeWorkspaceState w)

let workspaceFromWire (s: string) : Result<WorkspaceState, string> =
    parse s |> Result.bind decodeWorkspaceState
