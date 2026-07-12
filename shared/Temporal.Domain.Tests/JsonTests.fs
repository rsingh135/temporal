module Temporal.Domain.Tests.JsonTests

open Xunit
open Temporal.Domain.Json

let private ok (r: Result<'a, string>) : 'a =
    match r with
    | Ok v -> v
    | Error e -> failwith ("expected Ok, got Error: " + e)

let private err (r: Result<'a, string>) : string =
    match r with
    | Ok _ -> failwith "expected Error, got Ok"
    | Error e -> e

// --- number formatting -----------------------------------------------------

[<Theory>]
[<InlineData(0.0, "0")>]
[<InlineData(1.0, "1")>]
[<InlineData(-1.0, "-1")>]
[<InlineData(0.5, "0.5")>]
[<InlineData(-0.5, "-0.5")>]
[<InlineData(875.5, "875.5")>]
[<InlineData(0.000001, "0.000001")>]
[<InlineData(123456.789012, "123456.789012")>]
[<InlineData(1752300000000.0, "1752300000000")>]
[<InlineData(9007199254740991.0, "9007199254740991")>]
[<InlineData(0.1, "0.1")>]
[<InlineData(100.25, "100.25")>]
let ``formatNumber goldens`` (input: float) (expected: string) =
    Assert.Equal(expected, formatNumber input)

[<Fact>]
let ``formatNumber truncates beyond six decimals`` () =
    Assert.Equal("0.123457", formatNumber 0.1234567)

[<Fact>]
let ``negative zero prints as zero`` () =
    Assert.Equal("0", formatNumber -0.0)

// --- printing --------------------------------------------------------------

[<Fact>]
let ``prints compact deterministic output`` () =
    let v = JObject [ "a", JArray [ JNumber 1.0; JBool true; JNull ]; "b", JString "x" ]
    Assert.Equal("""{"a":[1,true,null],"b":"x"}""", print v)

[<Fact>]
let ``escapes control characters and quotes`` () =
    Assert.Equal("\"q\\\" b\\\\ n\\n t\\t c\\u0001\"", print (JString "q\" b\\ n\n t\t c\u0001"))

[<Fact>]
let ``non-ascii passes through raw`` () =
    Assert.Equal("\"日本語 🚀 é\"", print (JString "日本語 🚀 é"))

// --- parsing ---------------------------------------------------------------

[<Fact>]
let ``parses whitespace-tolerant json`` () =
    let v = ok (parse " { \"a\" : [ 1 , 2.5 , \"x\" ] , \"b\" : null } ")
    Assert.Equal(JObject [ "a", JArray [ JNumber 1.0; JNumber 2.5; JString "x" ]; "b", JNull ], v)

[<Fact>]
let ``print then parse is identity on nested values`` () =
    let v =
        JObject
            [ "s", JString "quote\" \\ \n \u0001 日本語 🚀"
              "n", JArray [ JNumber -0.5; JNumber 1752300000000.0; JNumber 0.000001 ]
              "o", JObject [ "empty", JArray []; "b", JBool false ] ]
    Assert.Equal(v, ok (parse (print v)))

[<Fact>]
let ``parse handles bmp unicode escapes`` () =
    Assert.Equal(JString "A é", ok (parse "\"\\u0041 \\u00e9\""))

[<Theory>]
[<InlineData("1e5")>]
[<InlineData("1E5")>]
[<InlineData("1.5e-3")>]
let ``rejects exponent notation`` (input: string) =
    Assert.Contains("exponent", err (parse input))

[<Fact>]
let ``rejects surrogate escapes`` () =
    Assert.Contains("surrogate", err (parse "\"\\ud83d\\ude00\""))

[<Theory>]
[<InlineData("")>]
[<InlineData("{\"a\":}")>]
[<InlineData("[1,]")>]
[<InlineData("{\"a\" 1}")>]
[<InlineData("\"unterminated")>]
[<InlineData("trux")>]
[<InlineData("1.5 trailing")>]
[<InlineData("1.")>]
let ``rejects malformed input`` (input: string) =
    match parse input with
    | Error _ -> ()
    | Ok v -> failwith ("expected parse failure but got: " + print v)

[<Fact>]
let ``rejects unescaped control characters`` () =
    Assert.Contains("control", err (parse "\"a\u0001b\""))
