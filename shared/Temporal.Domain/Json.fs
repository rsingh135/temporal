module Temporal.Domain.Json

open System.Text

/// Minimal JSON value model. This module is the wire format's single source of
/// truth: the same F# compiles to .NET, Rust and TypeScript, so every runtime
/// shares a byte-identical printer and parser.
///
/// Dialect notes (we control both ends of the wire):
///   - Printer emits no whitespace and never emits exponent notation.
///   - Non-integral numbers are printed with fixed-point precision of 6
///     decimal places (trailing zeros trimmed).
///   - Non-ASCII characters pass through raw (UTF-8); only quotes, backslash
///     and control characters are escaped.
///   - Parser rejects exponent notation and \uXXXX surrogate escapes, which
///     the printer never produces.
type JsonValue =
    | JNull
    | JBool of bool
    | JNumber of float
    | JString of string
    | JArray of JsonValue list
    | JObject of (string * JsonValue) list

// ---------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------

let private hexDigit (n: int) : char =
    if n < 10 then char (int '0' + n) else char (int 'a' + (n - 10))

/// Deterministic number formatting shared by all targets: integral values
/// print as integers; everything else as fixed-point with 6 decimal places,
/// computed with integer arithmetic so no runtime float-to-string is involved.
let formatNumber (v: float) : string =
    let neg = v < 0.0
    let av = if neg then -v else v
    let sign = if neg then "-" else ""
    if floor av = av then
        // Floats >= 2^53 are always integral, so this branch covers them all;
        // clamp to the i64 range (far beyond any value in the domain).
        let capped = if av > 9.2e18 then 9.2e18 else av
        let i = int64 capped
        if i = 0L then "0" else sign + string i
    else
        // Non-integral floats are < 2^53, so av * 1e6 fits i64 exactly enough.
        let r = int64 (floor (av * 1000000.0 + 0.5))
        let ip = r / 1000000L
        let fp = r % 1000000L
        if fp = 0L then sign + string ip
        else
            // 1000000 + fp is always 7 digits: drop the leading "1" to get a
            // zero-padded 6-digit fraction without format specifiers.
            let frac = (string (1000000L + fp)).Substring(1)
            let mutable e = frac.Length
            while e > 1 && frac.[e - 1] = '0' do
                e <- e - 1
            sign + string ip + "." + frac.Substring(0, e)

let private appendEscaped (sb: StringBuilder) (s: string) =
    let chars = s.ToCharArray()
    for i in 0 .. chars.Length - 1 do
        let c = chars.[i]
        if c = '"' then sb.Append("\\\"") |> ignore
        elif c = '\\' then sb.Append("\\\\") |> ignore
        elif c = '\n' then sb.Append("\\n") |> ignore
        elif c = '\r' then sb.Append("\\r") |> ignore
        elif c = '\t' then sb.Append("\\t") |> ignore
        elif c = '\b' then sb.Append("\\b") |> ignore
        elif c = '\f' then sb.Append("\\f") |> ignore
        elif int c < 32 then
            sb.Append("\\u00") |> ignore
            sb.Append(string (hexDigit ((int c >>> 4) &&& 15))) |> ignore
            sb.Append(string (hexDigit (int c &&& 15))) |> ignore
        else
            sb.Append(string c) |> ignore

let rec private appendValue (sb: StringBuilder) (v: JsonValue) =
    match v with
    | JNull -> sb.Append("null") |> ignore
    | JBool true -> sb.Append("true") |> ignore
    | JBool false -> sb.Append("false") |> ignore
    | JNumber n -> sb.Append(formatNumber n) |> ignore
    | JString s ->
        sb.Append("\"") |> ignore
        appendEscaped sb s
        sb.Append("\"") |> ignore
    | JArray items ->
        sb.Append("[") |> ignore
        let mutable first = true
        for item in items do
            if not first then sb.Append(",") |> ignore
            first <- false
            appendValue sb item
        sb.Append("]") |> ignore
    | JObject fields ->
        sb.Append("{") |> ignore
        let mutable first = true
        for (name, value) in fields do
            if not first then sb.Append(",") |> ignore
            first <- false
            sb.Append("\"") |> ignore
            appendEscaped sb name
            sb.Append("\":") |> ignore
            appendValue sb value
        sb.Append("}") |> ignore

/// Prints compact, deterministic JSON (no whitespace, codec-defined field order).
let print (v: JsonValue) : string =
    let sb = StringBuilder()
    appendValue sb v
    sb.ToString()

// ---------------------------------------------------------------------------
// Parsing (functional recursive descent; positions are indices into a char[])
// ---------------------------------------------------------------------------

let private isDigit (c: char) = c >= '0' && c <= '9'

let private isWs (c: char) =
    c = ' ' || c = '\t' || c = '\n' || c = '\r'

let rec private skipWs (cs: char[]) (pos: int) : int =
    if pos < cs.Length && isWs cs.[pos] then skipWs cs (pos + 1) else pos

let private errAt (pos: int) (message: string) : Result<'T, string> =
    Error (message + " at position " + string pos)

let private expectLiteral (cs: char[]) (pos: int) (lit: string) (value: JsonValue) : Result<JsonValue * int, string> =
    let lits = lit.ToCharArray()
    if pos + lits.Length <= cs.Length then
        let mutable ok = true
        for i in 0 .. lits.Length - 1 do
            if cs.[pos + i] <> lits.[i] then ok <- false
        if ok then Ok (value, pos + lits.Length)
        else errAt pos ("expected '" + lit + "'")
    else
        errAt pos ("expected '" + lit + "'")

let private hexValue (c: char) : int =
    if c >= '0' && c <= '9' then int c - int '0'
    elif c >= 'a' && c <= 'f' then int c - int 'a' + 10
    elif c >= 'A' && c <= 'F' then int c - int 'A' + 10
    else -1

let private parseString (cs: char[]) (start: int) : Result<string * int, string> =
    // start points at the opening quote.
    let sb = StringBuilder()
    let mutable pos = start + 1
    let mutable result: Result<string * int, string> option = None
    while result.IsNone do
        if pos >= cs.Length then
            result <- Some (errAt pos "unterminated string")
        else
            let c = cs.[pos]
            if c = '"' then
                result <- Some (Ok (sb.ToString(), pos + 1))
            elif c = '\\' then
                if pos + 1 >= cs.Length then
                    result <- Some (errAt pos "unterminated escape")
                else
                    let e = cs.[pos + 1]
                    if e = '"' then sb.Append("\"") |> ignore; pos <- pos + 2
                    elif e = '\\' then sb.Append("\\") |> ignore; pos <- pos + 2
                    elif e = '/' then sb.Append("/") |> ignore; pos <- pos + 2
                    elif e = 'n' then sb.Append("\n") |> ignore; pos <- pos + 2
                    elif e = 'r' then sb.Append("\r") |> ignore; pos <- pos + 2
                    elif e = 't' then sb.Append("\t") |> ignore; pos <- pos + 2
                    elif e = 'b' then sb.Append("\b") |> ignore; pos <- pos + 2
                    elif e = 'f' then sb.Append("\f") |> ignore; pos <- pos + 2
                    elif e = 'u' then
                        if pos + 5 >= cs.Length then
                            result <- Some (errAt pos "truncated \\u escape")
                        else
                            let h0 = hexValue cs.[pos + 2]
                            let h1 = hexValue cs.[pos + 3]
                            let h2 = hexValue cs.[pos + 4]
                            let h3 = hexValue cs.[pos + 5]
                            if h0 < 0 || h1 < 0 || h2 < 0 || h3 < 0 then
                                result <- Some (errAt pos "invalid \\u escape")
                            else
                                let code = (h0 <<< 12) ||| (h1 <<< 8) ||| (h2 <<< 4) ||| h3
                                if code >= 0xD800 && code <= 0xDFFF then
                                    // Our printer never emits surrogate escapes;
                                    // reject uniformly on every target.
                                    result <- Some (errAt pos "surrogate \\u escapes are not supported")
                                else
                                    sb.Append(string (char code)) |> ignore
                                    pos <- pos + 6
                    else
                        result <- Some (errAt pos "invalid escape")
            elif int c < 32 then
                result <- Some (errAt pos "unescaped control character in string")
            else
                sb.Append(string c) |> ignore
                pos <- pos + 1
    match result with
    | Some r -> r
    | None -> errAt start "unreachable"

let private parseNumber (cs: char[]) (start: int) : Result<JsonValue * int, string> =
    let mutable pos = start
    let neg = pos < cs.Length && cs.[pos] = '-'
    if neg then pos <- pos + 1
    if pos >= cs.Length || not (isDigit cs.[pos]) then
        errAt pos "expected digit"
    else
        let mutable ip = 0.0
        while pos < cs.Length && isDigit cs.[pos] do
            ip <- ip * 10.0 + float (int cs.[pos] - int '0')
            pos <- pos + 1
        let mutable value = ip
        let mutable bad = false
        if pos < cs.Length && cs.[pos] = '.' then
            pos <- pos + 1
            if pos >= cs.Length || not (isDigit cs.[pos]) then
                bad <- true
            else
                let mutable f = 0.0
                let mutable scale = 1.0
                while pos < cs.Length && isDigit cs.[pos] do
                    f <- f * 10.0 + float (int cs.[pos] - int '0')
                    scale <- scale * 10.0
                    pos <- pos + 1
                value <- ip + f / scale
        if bad then
            errAt pos "expected digit after decimal point"
        elif pos < cs.Length && (cs.[pos] = 'e' || cs.[pos] = 'E') then
            errAt pos "exponent notation is not supported"
        else
            Ok (JNumber (if neg then -value else value), pos)

let rec private parseValue (cs: char[]) (pos0: int) : Result<JsonValue * int, string> =
    let pos = skipWs cs pos0
    if pos >= cs.Length then errAt pos "unexpected end of input"
    else
        let c = cs.[pos]
        if c = '{' then parseObject cs (pos + 1)
        elif c = '[' then parseArray cs (pos + 1)
        elif c = '"' then
            match parseString cs pos with
            | Ok (s, next) -> Ok (JString s, next)
            | Error e -> Error e
        elif c = 't' then expectLiteral cs pos "true" (JBool true)
        elif c = 'f' then expectLiteral cs pos "false" (JBool false)
        elif c = 'n' then expectLiteral cs pos "null" JNull
        elif c = '-' || isDigit c then parseNumber cs pos
        else errAt pos "unexpected character"

and private parseArray (cs: char[]) (pos0: int) : Result<JsonValue * int, string> =
    let pos = skipWs cs pos0
    if pos < cs.Length && cs.[pos] = ']' then Ok (JArray [], pos + 1)
    else
        let rec loop (acc: JsonValue list) (p: int) : Result<JsonValue * int, string> =
            match parseValue cs p with
            | Error e -> Error e
            | Ok (item, next) ->
                let n = skipWs cs next
                if n < cs.Length && cs.[n] = ',' then loop (item :: acc) (n + 1)
                elif n < cs.Length && cs.[n] = ']' then Ok (JArray (List.rev (item :: acc)), n + 1)
                else errAt n "expected ',' or ']'"
        loop [] pos

and private parseObject (cs: char[]) (pos0: int) : Result<JsonValue * int, string> =
    let pos = skipWs cs pos0
    if pos < cs.Length && cs.[pos] = '}' then Ok (JObject [], pos + 1)
    else
        let rec loop (acc: (string * JsonValue) list) (p0: int) : Result<JsonValue * int, string> =
            let p = skipWs cs p0
            if p >= cs.Length || cs.[p] <> '"' then errAt p "expected field name"
            else
                match parseString cs p with
                | Error e -> Error e
                | Ok (name, afterName) ->
                    let colon = skipWs cs afterName
                    if colon >= cs.Length || cs.[colon] <> ':' then errAt colon "expected ':'"
                    else
                        match parseValue cs (colon + 1) with
                        | Error e -> Error e
                        | Ok (value, next) ->
                            let n = skipWs cs next
                            if n < cs.Length && cs.[n] = ',' then loop ((name, value) :: acc) (n + 1)
                            elif n < cs.Length && cs.[n] = '}' then Ok (JObject (List.rev ((name, value) :: acc)), n + 1)
                            else errAt n "expected ',' or '}'"
        loop [] pos

/// Parses a complete JSON document; trailing non-whitespace is an error.
let parse (s: string) : Result<JsonValue, string> =
    let cs = s.ToCharArray()
    match parseValue cs 0 with
    | Error e -> Error e
    | Ok (v, next) ->
        let rest = skipWs cs next
        if rest < cs.Length then errAt rest "unexpected trailing content"
        else Ok v
