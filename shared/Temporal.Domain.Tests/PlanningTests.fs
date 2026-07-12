module Temporal.Domain.Tests.PlanningTests

open Xunit
open Temporal.Domain.Planning
open Temporal.Parity

[<Fact>]
let ``toggle adds then removes`` () =
    let e1 = toggleExcluded "n2" []
    Assert.True(isExcluded "n2" e1)
    let e2 = toggleExcluded "n2" e1
    Assert.False(isExcluded "n2" e2)
    Assert.Empty(e2)

[<Fact>]
let ``included nodes drop excluded ones`` () =
    let ws = Fixtures.richWorkspace
    let included = includedNodes ws [ "n2"; "n4" ]
    Assert.Equal(List.length ws.Nodes - 2, List.length included)
    Assert.DoesNotContain(included, fun n -> n.NodeId = "n2")

[<Fact>]
let ``payload drops stale exclusions`` () =
    let payload = buildPayload Fixtures.richWorkspace [ "n2"; "ghost" ]
    Assert.Equal<string list>([ "n2" ], payload.ExcludedNodeIds)

[<Fact>]
let ``node detail summarizes payloads`` () =
    let ws = Fixtures.richWorkspace
    let byId id = ws.Nodes |> List.find (fun n -> n.NodeId = id)
    Assert.Equal("2 tabs", nodeDetail (byId "n1"))
    Assert.Equal("/Users/dev/temporal", nodeDetail (byId "n2"))
    Assert.Equal("/Users/dev/temporal", nodeDetail (byId "n3"))
