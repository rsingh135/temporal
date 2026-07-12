module Temporal.Domain.Types

/// Geometry of a single window in global (screen) coordinates, in points.
type WindowGeometry =
    { X: float
      Y: float
      Width: float
      Height: float }

/// Which adapter captured (and can rehydrate) a node.
type AdapterKind =
    | Chrome
    | TerminalApp
    | VSCode
    | Cursor
    | Generic

type BrowserTab =
    { Url: string
      Title: string }

type TerminalTab =
    { Tty: string
      WorkingDirectory: string }

/// Adapter-specific state carried by a window node.
type NodePayload =
    | BrowserWindow of tabs: BrowserTab list * activeTabIndex: int
    | TerminalWindow of tabs: TerminalTab list
    | EditorWindow of folderPath: string * openFiles: string list
    | GenericWindow

/// One captured window: the unit the user can toggle in the staging preview.
type WindowNode =
    { NodeId: string
      BundleId: string
      AppName: string
      WindowTitle: string
      Geometry: WindowGeometry
      Adapter: AdapterKind
      Payload: NodePayload }

/// A frozen desktop: flat record, overwritten in place (no history).
type WorkspaceState =
    { WorkspaceId: string
      CapturedAtUnixMs: int64
      Summary: string
      Tags: string list
      Nodes: WindowNode list }

/// What the user approved in the staging preview.
type RehydrationPayload =
    { Workspace: WorkspaceState
      ExcludedNodeIds: string list }

/// One semantic search hit; Score is cosine similarity in [0, 1].
type QueryCandidate =
    { Workspace: WorkspaceState
      Score: float }

/// UI -> daemon requests over the Unix domain socket.
type IpcRequest =
    | Ping
    | Freeze
    | Query of text: string * limit: int
    | Rehydrate of RehydrationPayload

/// Daemon -> UI responses. Progress may stream multiple times before Done.
type IpcResponse =
    | Pong
    | FreezeStarted of workspaceId: string
    | QueryResults of QueryCandidate list
    | RehydrateStarted
    | Progress of stage: string * detail: string * percent: int
    | Done of message: string
    | IpcError of code: string * message: string
