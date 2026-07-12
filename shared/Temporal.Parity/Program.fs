module Temporal.Parity.Program

open Temporal.Domain.Json

// Prints every fixture line, then re-parses and re-prints each one to prove
// parser parity. All three runtimes must produce byte-identical output.
[<EntryPoint>]
let main _argv =
    for line in Fixtures.lines do
        printfn "%s" line
    let mutable i = 0
    for line in Fixtures.lines do
        let status =
            match parse line with
            | Ok v -> if print v = line then "ok" else "MISMATCH"
            | Error e -> "ERROR " + e
        printfn "%s" ("reparse " + string i + " " + status)
        i <- i + 1
    0
