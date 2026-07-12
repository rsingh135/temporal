module Temporal.UI.Format

/// "just now" / "5m ago" / "3h ago" / "2d ago" for candidate rows.
let relativeAge (nowUnixMs: float) (capturedAtUnixMs: int64) : string =
    let deltaMs = nowUnixMs - float capturedAtUnixMs
    let minutes = int (deltaMs / 60000.0)
    if minutes < 1 then "just now"
    elif minutes < 60 then string minutes + "m ago"
    elif minutes < 60 * 24 then string (minutes / 60) + "h ago"
    else string (minutes / (60 * 24)) + "d ago"

/// 0.0-1.0 score to a percent label.
let scorePercent (score: float) : string =
    string (int (score * 100.0)) + "%"
