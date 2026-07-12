module Temporal.Domain.Tests.TaggingTests

open Xunit
open Temporal.Domain.Types
open Temporal.Domain.Tagging
open Temporal.Parity

[<Fact>]
let ``rich fixture derives project, host and app tags`` () =
    let tags = deriveTags Fixtures.richWorkspace
    Assert.Contains("temporal", tags)          // editor folder + terminal cwd
    Assert.Contains("fable.io", tags)          // url host
    Assert.Contains("github.com", tags)        // url host
    Assert.Contains("finder", tags)            // app name, lowercased
    Assert.Equal(tags.Length, (List.distinct tags).Length)

[<Fact>]
let ``summary mentions projects and tab count`` () =
    let summary = deriveSummary Fixtures.richWorkspace
    Assert.Contains("temporal", summary)
    Assert.Contains("2 browser tabs", summary)
    Assert.Contains("Finder", summary)

[<Fact>]
let ``empty workspace has empty-workspace summary and no tags`` () =
    Assert.Equal("empty workspace", deriveSummary Fixtures.emptyWorkspace)
    Assert.Empty(deriveTags Fixtures.emptyWorkspace)

[<Fact>]
let ``enrich fills tags and summary`` () =
    let enriched = enrich Fixtures.richWorkspace
    Assert.NotEmpty(enriched.Tags)
    Assert.NotEqual<string>("", enriched.Summary)
    Assert.Equal<WindowNode list>(Fixtures.richWorkspace.Nodes, enriched.Nodes)

[<Fact>]
let ``tagsToWire is canonical json`` () =
    let enriched = enrich Fixtures.emptyWorkspace
    Assert.Equal("[]", tagsToWire enriched)

[<Fact>]
let ``www prefix and ports are stripped from hosts`` () =
    let ws =
        { Fixtures.emptyWorkspace with
            Nodes =
                [ { NodeId = "n0"
                    BundleId = "com.google.Chrome"
                    AppName = "Google Chrome"
                    WindowTitle = ""
                    Geometry = { X = 0.0; Y = 0.0; Width = 1.0; Height = 1.0 }
                    Adapter = Chrome
                    Payload =
                      BrowserWindow (
                          [ { Url = "https://www.example.com/x"; Title = "" }
                            { Url = "http://localhost:3000/"; Title = "" } ],
                          0
                      ) } ] }
    let tags = deriveTags ws
    Assert.Contains("example.com", tags)
    Assert.Contains("localhost", tags)