module Temporal.Domain.Types

/// Geometry of a single window in global (screen) coordinates.
type WindowGeometry =
    { X: float
      Y: float
      Width: float
      Height: float }

/// Minimal M0 smoke type proving the F# -> Rust and F# -> TS pipelines.
type WorkspaceId = WorkspaceId of string

let workspaceIdValue (WorkspaceId id) = id

let geometryArea (g: WindowGeometry) : float = g.Width * g.Height
