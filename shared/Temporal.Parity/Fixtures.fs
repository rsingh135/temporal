module Temporal.Parity.Fixtures

open Temporal.Domain.Json
open Temporal.Domain.Types
open Temporal.Domain.Codecs

/// Wire-format fixtures exercised identically on .NET, Rust and TypeScript.
/// build/parity-test.sh diffs the printed output of all three runtimes; any
/// divergence in number formatting, escaping or field order shows up as a diff.

let private numberEdgeCases : JsonValue =
    JArray
        [ JNumber 0.0
          JNumber 1.0
          JNumber -1.0
          JNumber 0.5
          JNumber -0.5
          JNumber 875.5
          JNumber 0.000001
          JNumber -0.000001
          JNumber 123456.789012
          JNumber 2.675
          JNumber 1.0000005
          JNumber 1752300000000.0
          JNumber 9007199254740991.0
          JNumber 0.1
          JNumber 0.3
          JNumber -12345.678 ]

let private stringEdgeCases : JsonValue =
    JArray
        [ JString ""
          JString "plain ascii"
          JString "quote\" and back\\slash"
          JString "line\nbreak tab\t cr\r end"
          JString "ctrl\u0001char and\u001fdelim"
          JString "emoji 🚀 rocket 👩‍💻 zwj"
          JString "日本語テキスト"
          JString "café · naïve — dash" ]

let private chromeNode : WindowNode =
    { NodeId = "n1"
      BundleId = "com.google.Chrome"
      AppName = "Google Chrome"
      WindowTitle = "Fable · docs"
      Geometry = { X = 0.0; Y = 25.0; Width = 1440.0; Height = 875.5 }
      Adapter = Chrome
      Payload =
        BrowserWindow (
            [ { Url = "https://fable.io/docs/"; Title = "Fable · docs" }
              { Url = "https://github.com/fable-compiler/Fable"; Title = "GitHub — \"Fable\"" } ],
            1
        ) }

let private terminalNode : WindowNode =
    { NodeId = "n2"
      BundleId = "com.apple.Terminal"
      AppName = "Terminal"
      WindowTitle = "temporal — zsh"
      Geometry = { X = 720.5; Y = 300.0; Width = 640.0; Height = 480.0 }
      Adapter = TerminalApp
      Payload = TerminalWindow [ { Tty = "/dev/ttys003"; WorkingDirectory = "/Users/dev/temporal" } ] }

let private editorNode : WindowNode =
    { NodeId = "n3"
      BundleId = "com.microsoft.VSCode"
      AppName = "Visual Studio Code"
      WindowTitle = "temporal"
      Geometry = { X = -1440.0; Y = -875.0; Width = 1440.0; Height = 875.0 }
      Adapter = VSCode
      Payload = EditorWindow ("/Users/dev/temporal", [ "shared/Temporal.Domain/Json.fs"; "README.md" ]) }

let private genericNode : WindowNode =
    { NodeId = "n4"
      BundleId = "com.apple.finder"
      AppName = "Finder"
      WindowTitle = ""
      Geometry = { X = 100.25; Y = 200.75; Width = 800.0; Height = 600.0 }
      Adapter = Generic
      Payload = GenericWindow }

let richWorkspace : WorkspaceState =
    { WorkspaceId = "ws-rich"
      CapturedAtUnixMs = 1752300000000L
      Summary = "debugging the rust daemon & codecs"
      Tags = [ "rust"; "temporal"; "fable"; "日本語" ]
      Nodes = [ chromeNode; terminalNode; editorNode; genericNode ] }

let emptyWorkspace : WorkspaceState =
    { WorkspaceId = "ws-empty"
      CapturedAtUnixMs = 0L
      Summary = ""
      Tags = []
      Nodes = [] }

let lines : string list =
    [ print numberEdgeCases
      print stringEdgeCases
      workspaceToWire richWorkspace
      workspaceToWire emptyWorkspace
      requestToWire Ping
      requestToWire Freeze
      requestToWire (Query ("that rust daemon work from tuesday", 5))
      requestToWire (Rehydrate { Workspace = richWorkspace; ExcludedNodeIds = [ "n2"; "n4" ] })
      responseToWire Pong
      responseToWire (FreezeStarted "ws-7")
      responseToWire (QueryResults [ { Workspace = richWorkspace; Score = 0.873512 }
                                     { Workspace = emptyWorkspace; Score = 0.5 } ])
      responseToWire RehydrateStarted
      responseToWire (Progress ("extract", "Google Chrome", 42))
      responseToWire (Done "rehydrated 4 nodes")
      responseToWire (IpcError ("E_TCC", "accessibility permission not granted")) ]
