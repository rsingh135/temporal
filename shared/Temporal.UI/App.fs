module Temporal.UI.App

open Fable.Core
open Fable.Core.JsInterop
open Feliz
open Temporal.Domain.Types
open Temporal.Domain.Codecs
open Temporal.Domain.Planning
open Temporal.UI.Format

// ---------------------------------------------------------------------------
// Tauri interop
// ---------------------------------------------------------------------------

[<Import("invoke", "@tauri-apps/api/core")>]
let private tauriInvoke (command: string) (args: obj) : JS.Promise<string array> = jsNative

[<Import("listen", "@tauri-apps/api/event")>]
let private tauriListen (event: string) (handler: obj -> unit) : JS.Promise<unit -> unit> = jsNative

/// Sends one IPC request through the Tauri shell; decodes every response
/// frame with the shared codec.
let private sendRequest (request: IpcRequest) : JS.Promise<IpcResponse list> =
    tauriInvoke "daemon_request" {| requestJson = requestToWire request |}
    |> Promise.map (fun frames ->
        frames
        |> Array.toList
        |> List.map (fun frame ->
            match responseFromWire frame with
            | Ok response -> response
            | Error e -> IpcError ("E_DECODE", e)))

let private hidePanel () : unit =
    tauriInvoke "hide_panel" (createObj []) |> ignore

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

type private Phase =
    | Searching
    | Staging of QueryCandidate
    | Busy of string
    | Feedback of ok: bool * message: string

let private adapterGlyph (adapter: AdapterKind) : string =
    match adapter with
    | Chrome -> "🌐"
    | TerminalApp -> "🖥"
    | VSCode | Cursor -> "📝"
    | Generic -> "▫️"

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

[<ReactComponent>]
let private CandidateRow (candidate: QueryCandidate) (selected: bool) (onPick: unit -> unit) =
    let w = candidate.Workspace
    Html.div [
        prop.className (if selected then "candidate selected" else "candidate")
        prop.onClick (fun _ -> onPick ())
        prop.children [
            Html.div [
                prop.className "candidate-main"
                prop.children [
                    Html.span [ prop.className "candidate-summary"; prop.text w.Summary ]
                    Html.span [
                        prop.className "candidate-meta"
                        prop.text (
                            relativeAge (emitJsExpr () "Date.now()") w.CapturedAtUnixMs
                            + " · " + string (List.length w.Nodes) + " windows"
                            + " · " + scorePercent candidate.Score
                        )
                    ]
                ]
            ]
            Html.div [
                prop.className "candidate-tags"
                prop.text (String.concat "  " (List.truncate 6 w.Tags))
            ]
        ]
    ]

[<ReactComponent>]
let private NodeRow (node: WindowNode) (excluded: bool) (onToggle: unit -> unit) =
    Html.label [
        prop.className (if excluded then "node excluded" else "node")
        prop.children [
            Html.input [
                prop.type' "checkbox"
                prop.isChecked (not excluded)
                prop.onChange (fun (_: bool) -> onToggle ())
            ]
            Html.span [ prop.className "node-glyph"; prop.text (adapterGlyph node.Adapter) ]
            Html.div [
                prop.className "node-text"
                prop.children [
                    Html.span [ prop.className "node-title"; prop.text (node.AppName + " — " + node.WindowTitle) ]
                    Html.span [ prop.className "node-detail"; prop.text (nodeDetail node) ]
                ]
            ]
        ]
    ]

[<ReactComponent>]
let App () =
    let query, setQuery = React.useState ""
    let candidates, setCandidates = React.useState ([]: QueryCandidate list)
    let phase, setPhase = React.useState Searching
    let excluded, setExcluded = React.useState ([]: string list)
    let selectedIndex, setSelectedIndex = React.useState 0
    let inputRef = React.useInputRef ()

    let runQuery (text: string) =
        sendRequest (Query (text, 8))
        |> Promise.iter (fun responses ->
            match responses with
            | [ QueryResults results ] ->
                setCandidates results
                setSelectedIndex 0
            | [ IpcError (_, message) ] -> setPhase (Feedback (false, message))
            | _ -> ())

    // Debounced search-as-you-type.
    React.useEffect (
        (fun () ->
            let id = JS.setTimeout (fun () -> runQuery query) 200
            let cleanup () = JS.clearTimeout id
            cleanup),
        [| box query |]
    )

    // Panel re-shown via hotkey: reset to a fresh search.
    React.useEffectOnce (fun () ->
        tauriListen "panel-shown" (fun _ ->
            setPhase Searching
            setExcluded []
            setQuery ""
            runQuery ""
            inputRef.current |> Option.iter (fun el -> el.focus ()))
        |> ignore)

    let freeze () =
        setPhase (Busy "Freezing current desktop…")
        sendRequest Freeze
        |> Promise.iter (fun responses ->
            match List.tryLast responses with
            | Some (Done message) -> setPhase (Feedback (true, message))
            | Some (IpcError (_, message)) -> setPhase (Feedback (false, message))
            | _ -> setPhase (Feedback (false, "unexpected daemon response")))

    let rehydrate (candidate: QueryCandidate) =
        setPhase (Busy "Rehydrating…")
        sendRequest (Rehydrate (buildPayload candidate.Workspace excluded))
        |> Promise.iter (fun responses ->
            match List.tryLast responses with
            | Some (Done message) -> setPhase (Feedback (true, message))
            | Some (IpcError (_, message)) -> setPhase (Feedback (false, message))
            | _ -> setPhase (Feedback (false, "unexpected daemon response")))

    let onSearchKey (key: string) =
        match key with
        | "Escape" -> hidePanel ()
        | "ArrowDown" ->
            setSelectedIndex (min (selectedIndex + 1) (List.length candidates - 1))
        | "ArrowUp" -> setSelectedIndex (max (selectedIndex - 1) 0)
        | "Enter" ->
            match List.tryItem selectedIndex candidates with
            | Some candidate ->
                setExcluded []
                setPhase (Staging candidate)
            | None -> ()
        | _ -> ()

    Html.div [
        prop.className "panel"
        prop.children [
            match phase with
            | Searching ->
                Html.input [
                    prop.className "omnibar"
                    prop.ref inputRef
                    prop.autoFocus true
                    prop.placeholder "Summon a workspace…  (⌥Space to dismiss)"
                    prop.value query
                    prop.onChange (fun (v: string) -> setQuery v)
                    prop.onKeyDown (fun ev -> onSearchKey ev.key)
                ]
                Html.div [
                    prop.className "candidates"
                    prop.children [
                        for i, candidate in List.indexed candidates ->
                            CandidateRow candidate (i = selectedIndex) (fun () ->
                                setExcluded []
                                setPhase (Staging candidate))
                    ]
                ]
                Html.div [
                    prop.className "footer"
                    prop.children [
                        Html.span [ prop.className "hint"; prop.text "↑↓ select · ⏎ stage · esc dismiss" ]
                        Html.button [
                            prop.className "freeze"
                            prop.text "❄ Freeze now"
                            prop.onClick (fun _ -> freeze ())
                        ]
                    ]
                ]
            | Staging candidate ->
                let w = candidate.Workspace
                Html.div [
                    prop.className "staging-header"
                    prop.children [
                        Html.span [ prop.className "staging-summary"; prop.text w.Summary ]
                        Html.span [
                            prop.className "staging-meta"
                            prop.text (relativeAge (emitJsExpr () "Date.now()") w.CapturedAtUnixMs)
                        ]
                    ]
                ]
                Html.div [
                    prop.className "nodes"
                    prop.children [
                        for node in w.Nodes ->
                            NodeRow node (isExcluded node.NodeId excluded) (fun () ->
                                setExcluded (toggleExcluded node.NodeId excluded))
                    ]
                ]
                Html.div [
                    prop.className "footer"
                    prop.children [
                        Html.button [
                            prop.className "back"
                            prop.text "← Back"
                            prop.onClick (fun _ -> setPhase Searching)
                        ]
                        Html.button [
                            prop.className "approve"
                            prop.text (
                                "Rehydrate "
                                + string (List.length (includedNodes w excluded))
                                + " windows"
                            )
                            prop.onClick (fun _ -> rehydrate candidate)
                        ]
                    ]
                ]
            | Busy label ->
                Html.div [ prop.className "busy"; prop.text label ]
            | Feedback (ok, message) ->
                Html.div [
                    prop.className (if ok then "feedback ok" else "feedback err")
                    prop.text message
                ]
                Html.div [
                    prop.className "footer"
                    prop.children [
                        Html.button [
                            prop.className "back"
                            prop.text "← Back to search"
                            prop.onClick (fun _ -> setPhase Searching)
                        ]
                    ]
                ]
        ]
    ]
