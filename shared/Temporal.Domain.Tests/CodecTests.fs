module Temporal.Domain.Tests.CodecTests

open Xunit
open Temporal.Domain.Types
open Temporal.Domain.Codecs
open Temporal.Parity

let private ok (r: Result<'a, string>) : 'a =
    match r with
    | Ok v -> v
    | Error e -> failwith ("expected Ok, got Error: " + e)

let private err (r: Result<'a, string>) : string =
    match r with
    | Ok _ -> failwith "expected Error, got Ok"
    | Error e -> e

[<Fact>]
let ``workspace decode-encode roundtrips the rich fixture`` () =
    let decoded = ok (workspaceFromWire (workspaceToWire Fixtures.richWorkspace))
    Assert.Equal(Fixtures.richWorkspace, decoded)

[<Fact>]
let ``workspace decode-encode roundtrips the empty fixture`` () =
    let decoded = ok (workspaceFromWire (workspaceToWire Fixtures.emptyWorkspace))
    Assert.Equal(Fixtures.emptyWorkspace, decoded)

[<Fact>]
let ``all ipc requests roundtrip`` () =
    let requests =
        [ Ping
          Freeze
          Query ("rust daemon work", 8)
          Rehydrate { Workspace = Fixtures.richWorkspace; ExcludedNodeIds = [ "n1" ] } ]
    for r in requests do
        Assert.Equal(r, ok (requestFromWire (requestToWire r)))

[<Fact>]
let ``all ipc responses roundtrip`` () =
    let responses =
        [ Pong
          FreezeStarted "ws-1"
          QueryResults [ { Workspace = Fixtures.richWorkspace; Score = 0.998877 } ]
          RehydrateStarted
          Progress ("launch", "Terminal", 90)
          Done "ok"
          IpcError ("E_IO", "socket closed") ]
    for r in responses do
        Assert.Equal(r, ok (responseFromWire (responseToWire r)))

[<Fact>]
let ``missing field reports its name`` () =
    Assert.Contains("workspaceId", err (workspaceFromWire "{}"))

[<Fact>]
let ``unknown adapter kind is rejected`` () =
    let wire = (workspaceToWire Fixtures.richWorkspace).Replace("\"chrome\"", "\"netscape\"")
    Assert.Contains("adapter kind", err (workspaceFromWire wire))

[<Fact>]
let ``unknown request type is rejected`` () =
    Assert.Contains("unknown request type", err (requestFromWire "{\"type\":\"nuke\"}"))

[<Fact>]
let ``malformed json surfaces parse error`` () =
    Assert.Contains("position", err (workspaceFromWire "{\"workspaceId\":"))
