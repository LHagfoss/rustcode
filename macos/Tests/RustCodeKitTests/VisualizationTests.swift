import XCTest
@testable import RustCodeKit

final class VisualizationTests: XCTestCase {
    private let bar = #"{"version":1,"type":"bar_chart","title":"Build time","summary":"engine dominates","unit":"s","items":[{"label":"engine","value":42},{"label":"tui","value":10.5},{"label":"core","value":3}]}"#

    private func fenced(_ body: String) -> String {
        "Here is the breakdown:\n\n```rustcode-viz\n\(body)\n```\n\nDone."
    }

    private func reason(_ body: String) -> String? {
        do {
            _ = try Visualization.parse(body)
            return nil
        } catch {
            return (error as? Visualization.ParseError)?.description
        }
    }

    func testBarChartParsesWithTitleSummaryAndUnit() throws {
        let visualization = try Visualization.parse(bar)
        XCTAssertEqual(visualization.title, "Build time")
        XCTAssertEqual(visualization.summary, "engine dominates")
        guard case .barChart(let bars, let unit, let hidden) = visualization.content else { return XCTFail() }
        XCTAssertEqual(bars.map(\.label), ["engine", "tui", "core"])
        XCTAssertEqual(bars.map(\.value), [42, 10.5, 3])
        XCTAssertEqual(unit, "s")
        XCTAssertEqual(hidden, 0)
    }

    func testAssistantMarkdownYieldsAVisualizationWithItsSourceFence() {
        let blocks = MarkdownBlock.parse(fenced(bar), visualizations: true)
        XCTAssertEqual(blocks.count, 3)
        guard case .visualization(let block) = blocks[1] else { return XCTFail("\(blocks)") }
        XCTAssertEqual(block.source, "```rustcode-viz\n\(bar)\n```")
        XCTAssertEqual(block.body, bar)
        guard case .rendered = block.state else { return XCTFail("\(block.state)") }
        XCTAssertEqual(blocks[0], .paragraph("Here is the breakdown:"))
        XCTAssertEqual(blocks[2], .paragraph("Done."))
    }

    func testOtherTextKeepsTheFenceAsCode() {
        let blocks = MarkdownBlock.parse(fenced(bar))
        XCTAssertEqual(blocks[1], .code(language: "rustcode-viz", text: bar))
    }

    func testOrdinaryFencesAndTablesAreUnchanged() {
        let text = "```json\n{\"version\":1,\"type\":\"bar_chart\",\"items\":[]}\n```\n\n| a | b |\n| --- | --- |\n| 1 | 2 |"
        XCTAssertEqual(MarkdownBlock.parse(text, visualizations: true), [
            .code(language: "json", text: "{\"version\":1,\"type\":\"bar_chart\",\"items\":[]}"),
            .table(headers: ["a", "b"], rows: [["1", "2"]]),
        ])
        XCTAssertFalse(Visualization.isInfoString("rustcode-viz2"))
        XCTAssertTrue(Visualization.isInfoString(" rustcode-viz "))
    }

    func testAnUnclosedFenceIsIncompleteAndNeverParsed() {
        let partial = "Intro\n\n```rustcode-viz\n{\"version\":1,\"type\":\"bar_chart\",\"items\":["
        guard case .visualization(let block) = MarkdownBlock.parse(partial, visualizations: true).last else {
            return XCTFail()
        }
        XCTAssertEqual(block.state, .incomplete)
        XCTAssertEqual(block.body, "{\"version\":1,\"type\":\"bar_chart\",\"items\":[")
    }

    func testInvalidBlocksFallBackWithTheTUIReasons() {
        XCTAssertEqual(reason("{not json"), "invalid JSON")
        XCTAssertEqual(reason("[1,2]"), "payload is not a JSON object")
        XCTAssertEqual(reason(#"{"type":"bar_chart","items":[]}"#), "missing \"version\"")
        XCTAssertEqual(reason(#"{"version":2,"type":"bar_chart"}"#), "unsupported version 2")
        XCTAssertEqual(reason(#"{"version":1}"#), "missing \"type\"")
        XCTAssertEqual(reason(#"{"version":1,"type":"pie"}"#), "unsupported type \"pie\"")
        XCTAssertEqual(reason(#"{"version":1,"type":"bar_chart","items":[]}"#), "bar_chart has no items")
        XCTAssertEqual(reason(#"{"version":1,"type":"bar_chart","items":[{"label":"x","value":"big"}]}"#),
                       "every bar_chart item needs a finite numeric \"value\"")
        XCTAssertEqual(reason(#"{"version":1,"type":"table","columns":["a"],"rows":[[{"x":1}]]}"#),
                       "table cells must be strings, numbers, booleans, or null")
        XCTAssertEqual(reason(#"{"version":1,"type":"table","rows":[],"sort_by":{"column":"a"}}"#),
                       "table needs a \"columns\" array")
        XCTAssertEqual(reason(#"{"version":1,"type":"table","columns":["a"],"rows":[1],"sort_by":{"column":"a"}}"#),
                       "table rows must be arrays")
        XCTAssertEqual(reason(#"{"version":2,"type":"table","columns":["a"],"rows":[]}"#), "unsupported version 2")
        XCTAssertEqual(reason(#"{"version":1,"type":"tree","nodes":[{"label":"a","children":"b"}]}"#),
                       "tree \"children\" must be an array")
        XCTAssertEqual(reason(#"{"version":1,"type":"tree","title":5,"nodes":[{"label":"a"}]}"#),
                       "\"title\" must be a string")
        XCTAssertEqual(reason("   "), "empty block")

        guard case .visualization(let block) = MarkdownBlock.parse(fenced("{not json"), visualizations: true)[1] else {
            return XCTFail()
        }
        XCTAssertEqual(block.state, .invalid("invalid JSON"))
    }

    func testOversizedPayloadsFallBackAndLongListsTruncate() throws {
        let huge = #"{"version":1,"type":"bar_chart","items":[{"label":"\#(String(repeating: "x", count: 16 * 1024))","value":1}]}"#
        XCTAssertEqual(reason(huge), "payload is 17 KiB, limit is 16 KiB")

        let items = (0..<45).map { #"{"label":"item \#($0)","value":\#($0)}"# }.joined(separator: ",")
        guard case .barChart(let bars, _, let hidden) = try Visualization.parse(#"{"version":1,"type":"bar_chart","items":[\#(items)]}"#).content else {
            return XCTFail()
        }
        XCTAssertEqual(bars.count, Visualization.maxBarItems)
        XCTAssertEqual(hidden, 15)

        let rows = (0..<80).map { #"["r\#($0)",\#($0)]"# }.joined(separator: ",")
        let columns = (0..<12).map { #""c\#($0)""# }.joined(separator: ",")
        guard case .table(let headers, let tableRows, let numeric, _, _, let hiddenColumns) =
            try Visualization.parse(#"{"version":1,"type":"table","columns":[\#(columns)],"rows":[\#(rows)]}"#).content else {
            return XCTFail()
        }
        XCTAssertEqual(headers.count, Visualization.maxTableColumns)
        XCTAssertEqual(hiddenColumns, 4)
        // All rows stay so sorting covers the whole table; the view shows 50.
        XCTAssertEqual(tableRows.count, 80)
        XCTAssertEqual(numeric.prefix(2), [false, true])

        let leaves = (0..<200).map { #"{"label":"n\#($0)"}"# }.joined(separator: ",")
        guard case .tree(let nodes, let hiddenNodes) =
            try Visualization.parse(#"{"version":1,"type":"tree","nodes":[{"label":"root","children":[\#(leaves)]}]}"#).content else {
            return XCTFail()
        }
        XCTAssertEqual(nodes[0].children.count, Visualization.maxTreeNodes - 1)
        XCTAssertEqual(hiddenNodes, 81)

        var deep = #"{"label":"leaf"}"#
        for level in 0..<12 { deep = #"{"label":"l\#(level)","children":[\#(deep)]}"# }
        guard case .tree(_, let hiddenDeep) = try Visualization.parse(#"{"version":1,"type":"tree","nodes":[\#(deep)]}"#).content else {
            return XCTFail()
        }
        XCTAssertEqual(hiddenDeep, 13 - (Visualization.maxTreeDepth + 1))

        let long = String(repeating: "y", count: 500)
        let visualization = try Visualization.parse(#"{"version":1,"type":"bar_chart","title":"\#(long)","items":[{"label":"\#(long)","value":1}]}"#)
        XCTAssertEqual(visualization.title.count, Visualization.maxTitleCharacters)
        XCTAssertTrue(visualization.title.hasSuffix("…"))
        guard case .barChart(let longBars, _, _) = visualization.content else { return XCTFail() }
        XCTAssertEqual(longBars[0].label.count, Visualization.maxLabelCharacters)
    }

    func testControlCharactersBecomeSpaces() throws {
        let visualization = try Visualization.parse(#"{"version":1,"type":"tree","title":"a\u001b[31mred","nodes":[{"label":"x\u0007\nbell"}]}"#)
        XCTAssertEqual(visualization.title, "a [31mred")
        guard case .tree(let nodes, _) = visualization.content else { return XCTFail() }
        XCTAssertEqual(nodes[0].label, "x bell")
    }

    func testTableSortsNumbersByValueAndTextLexically() throws {
        let body = #"{"version":1,"type":"table","columns":["Test","ms"],"rows":[["b",5],["a",900],["c",40]],"sort_by":{"column":1,"descending":true}}"#
        guard case .table(_, let rows, let numeric, let column, let descending, _) = try Visualization.parse(body).content else {
            return XCTFail()
        }
        XCTAssertEqual(numeric, [false, true])
        XCTAssertEqual(column, 1)
        XCTAssertTrue(descending)
        let byValue = Visualization.Content.sortedRows(rows, column: 1, descending: true)
        XCTAssertEqual(byValue.map { $0[0].text }, ["a", "c", "b"])
        let byName = Visualization.Content.sortedRows(rows, column: 0, descending: false)
        XCTAssertEqual(byName.map { $0[0].text }, ["a", "b", "c"])
        XCTAssertEqual(Visualization.Content.sortedRows(rows, column: nil, descending: false).map { $0[0].text }, ["b", "a", "c"])
    }

    func testTableSortByAcceptsAnIndexANumericStringOrAColumnName() throws {
        func sort(_ column: String) throws -> (Int?, Bool) {
            let body = #"{"version":1,"type":"table","title":"Largest Rust source files","columns":["Source file","Lines"],"rows":[["b.rs",5],["a.rs",900],["c.rs",40]],"sort_by":{"column":"# + column + #","descending":true}}"#
            guard case .table(_, _, _, let column, let descending, _) = try Visualization.parse(body).content else {
                XCTFail()
                return (nil, false)
            }
            return (column, descending)
        }
        for column in ["1", #""1""#, #""Lines""#, #"" lines ""#] {
            let (resolved, descending) = try sort(column)
            XCTAssertEqual(resolved, 1, column)
            XCTAssertTrue(descending, column)
        }
        // An unknown or hidden column is only a hint: the table still renders unsorted.
        for column in [#""Size""#, "7", #""7""#, "true", "-1"] {
            XCTAssertNil(try sort(column).0, column)
        }
    }

    func testTreeNodesKeepNotesAndUniqueIds() throws {
        let body = #"{"version":1,"type":"tree","nodes":[{"label":"ui/","children":[{"label":"markdown.rs","note":"tables"},{"label":"modals/","children":[{"label":"theme.rs"}]}]}]}"#
        guard case .tree(let nodes, let hidden) = try Visualization.parse(body).content else { return XCTFail() }
        XCTAssertEqual(hidden, 0)
        XCTAssertEqual(nodes[0].children[0].note, "tables")
        XCTAssertEqual([nodes[0].id, nodes[0].children[0].id, nodes[0].children[1].id, nodes[0].children[1].children[0].id], [0, 1, 2, 3])
    }

    func testInitializeAdvertisesVisualizationLevelTwo() {
        let params = ACPRequests.initialize(appVersion: "1.0")
        XCTAssertEqual(params["clientCapabilities"]?["_meta"]?["rustcode"]?["visualizations"], .number(2))
        // The payload schema stays version 1; level 2 only adds types.
        XCTAssertEqual(Visualization.version, 1)
    }

    // MARK: - Level 2 widgets

    func testLineChartParsesLabelsSeriesGapsAndUnit() throws {
        let body = #"{"version":1,"type":"line_chart","title":"Size","unit":"MB","labels":["v1",2,"v3"],"series":[{"name":"mac","values":[1.5,null,3]},{"values":[2]}]}"#
        let visualization = try Visualization.parse(body)
        guard case .lineChart(let labels, let series, let unit, let hiddenPoints, let hiddenSeries) = visualization.content else {
            return XCTFail()
        }
        XCTAssertEqual(labels, ["v1", "2", "v3"])
        XCTAssertEqual(unit, "MB")
        XCTAssertEqual(series.map(\.name), ["mac", "series 2"])
        XCTAssertEqual(series[0].values, [1.5, nil, 3])
        // A short series is padded with gaps to the label count.
        XCTAssertEqual(series[1].values, [2, nil, nil])
        XCTAssertEqual(hiddenPoints, 0)
        XCTAssertEqual(hiddenSeries, 0)
        XCTAssertEqual(visualization.accessibilitySummary,
                       "Line chart, 3 points from v1 to v3. mac: 1.5 MB to 3 MB, low 1.5 MB, high 3 MB; series 2: 2 MB to 2 MB, low 2 MB, high 2 MB.")
    }

    func testLineChartLimitsPointsAndSeries() throws {
        let labels = (0..<130).map { "\($0)" }.joined(separator: ",")
        let values = (0..<130).map { "\($0)" }.joined(separator: ",")
        let series = (0..<6).map { #"{"name":"s\#($0)","values":[\#(values)]}"# }.joined(separator: ",")
        guard case .lineChart(let shownLabels, let shownSeries, _, let hiddenPoints, let hiddenSeries) =
            try Visualization.parse(#"{"version":1,"type":"line_chart","labels":[\#(labels)],"series":[\#(series)]}"#).content else {
            return XCTFail()
        }
        XCTAssertEqual(shownLabels.count, Visualization.maxLinePoints)
        XCTAssertEqual(shownSeries.count, Visualization.maxLineSeries)
        XCTAssertTrue(shownSeries.allSatisfy { $0.values.count == Visualization.maxLinePoints })
        XCTAssertEqual(hiddenPoints, 30)
        XCTAssertEqual(hiddenSeries, 2)
    }

    func testStatListParsesValuesUnitsAndNotes() throws {
        let body = #"{"version":1,"type":"stat_list","items":[{"label":"Tests","value":3104,"note":"all passing"},{"label":"Coverage","value":81.25,"unit":"%"},{"label":"LOC","value":"186k"}]}"#
        let visualization = try Visualization.parse(body)
        XCTAssertEqual(visualization.title, "stat list")
        guard case .statList(let items, let hidden) = visualization.content else { return XCTFail() }
        XCTAssertEqual(items.map(\.value), ["3104", "81.25 %", "186k"])
        XCTAssertEqual(items.map(\.number), [3104, 81.25, nil])
        XCTAssertEqual(items[0].note, "all passing")
        XCTAssertEqual(hidden, 0)
        XCTAssertEqual(visualization.accessibilitySummary, "Tests: 3104, Coverage: 81.25 %, LOC: 186k.")

        let many = (0..<20).map { #"{"label":"k\#($0)","value":\#($0)}"# }.joined(separator: ",")
        guard case .statList(let capped, let hiddenMany) =
            try Visualization.parse(#"{"version":1,"type":"stat_list","items":[\#(many)]}"#).content else { return XCTFail() }
        XCTAssertEqual(capped.count, Visualization.maxStatItems)
        XCTAssertEqual(hiddenMany, 8)
    }

    func testChecklistCountsEveryItemAndDefaultsToTodo() throws {
        let items = (0..<60).map { index in
            #"{"label":"step \#(index)","status":"\#(index < 45 ? "done" : "failed")"}"#
        }.joined(separator: ",")
        let visualization = try Visualization.parse(#"{"version":1,"type":"checklist","items":[\#(items),{"label":"last"}]}"#)
        guard case .checklist(let shown, let done, let total, let hidden) = visualization.content else { return XCTFail() }
        XCTAssertEqual(shown.count, Visualization.maxChecklistItems)
        XCTAssertEqual(done, 45)
        XCTAssertEqual(total, 61)
        XCTAssertEqual(hidden, 11)
        XCTAssertEqual(shown[0].status, .done)
        XCTAssertEqual(visualization.accessibilitySummary, "Checklist, 45 of 61 done, 5 failed.")

        let parsed = try Visualization.parse(#"{"version":1,"type":"checklist","items":[{"label":"a","status":"in_progress","note":"half"},{"label":"b"}]}"#)
        guard case .checklist(let pair, _, _, _) = parsed.content else { return XCTFail() }
        XCTAssertEqual(pair.map(\.status), [.inProgress, .todo])
        XCTAssertEqual(pair[0].note, "half")
    }

    func testLevelTwoWidgetsFallBackWithReasons() {
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","series":[]}"#), "line_chart needs a \"labels\" array")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":[],"series":[]}"#), "line_chart has no labels")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":["a"],"series":[]}"#), "line_chart has no series")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":[true],"series":[{"values":[1]}]}"#),
                       "line_chart labels must be strings or numbers")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":["a"],"series":[{"values":["1"]}]}"#),
                       "line_chart values must be finite numbers or null")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":["a"],"series":[{"name":3,"values":[1]}]}"#),
                       "line_chart series \"name\" must be a string")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":["a"],"series":[{"values":[null]}]}"#),
                       "line_chart has no values")
        XCTAssertEqual(reason(#"{"version":1,"type":"line_chart","labels":["a"],"series":[1]}"#),
                       "line_chart series must be objects")
        XCTAssertEqual(reason(#"{"version":1,"type":"stat_list","items":[]}"#), "stat_list has no items")
        XCTAssertEqual(reason(#"{"version":1,"type":"stat_list","items":[{"label":"a","value":[1]}]}"#),
                       "every stat_list item needs a string or finite numeric \"value\"")
        XCTAssertEqual(reason(#"{"version":1,"type":"stat_list","items":[{"value":1}]}"#),
                       "every stat_list item needs a string \"label\"")
        XCTAssertEqual(reason(#"{"version":1,"type":"checklist","items":[{"label":"a","status":"maybe"}]}"#),
                       "checklist \"status\" must be done, todo, in_progress, failed, or skipped")
        XCTAssertEqual(reason(#"{"version":1,"type":"checklist","items":[{"status":"done"}]}"#),
                       "every checklist item needs a string \"label\"")
        XCTAssertEqual(reason(#"{"version":1,"type":"checklist"}"#), "checklist needs an \"items\" array")
    }

    func testLevelTwoTextIsSingleLineAndBounded() throws {
        let long = String(repeating: "z", count: 200)
        let visualization = try Visualization.parse(#"{"version":1,"type":"checklist","items":[{"label":"a\u001b[2Jb\nc","note":"\#(long)"}]}"#)
        guard case .checklist(let items, _, _, _) = visualization.content else { return XCTFail() }
        XCTAssertEqual(items[0].label, "a [2Jb c")
        XCTAssertEqual(items[0].note?.count, Visualization.maxCellCharacters)
    }

    func testStreamingLevelTwoBlocksStayIncompleteUntilTheFenceCloses() {
        let body = #"{"version":1,"type":"line_chart","labels":["a","b"],"series":[{"values":[1,2]}]}"#
        let full = "Trend:\n\n```rustcode-viz\n\(body)\n```"
        // Every prefix that stops before the closing fence is a placeholder,
        // never a half-drawn widget or a parse error.
        let closing = full.range(of: "\n```", options: .backwards)!.lowerBound
        for end in stride(from: full.distance(from: full.startIndex, to: full.range(of: "rustcode-viz\n")!.upperBound),
                          to: full.distance(from: full.startIndex, to: closing), by: 7) {
            let partial = String(full.prefix(end))
            guard case .visualization(let block) = MarkdownBlock.parse(partial, visualizations: true).last else {
                return XCTFail("no block for \(partial)")
            }
            XCTAssertEqual(block.state, .incomplete, partial)
        }
        guard case .visualization(let done) = MarkdownBlock.parse(full, visualizations: true).last,
              case .rendered = done.state else { return XCTFail() }
    }

    func testPlainTextExportCoversEveryType() throws {
        let table = try Visualization.parse(#"{"version":1,"type":"table","title":"T","columns":["a|b","n"],"rows":[["x",2],["y",1]],"sort_by":{"column":1}}"#)
        XCTAssertEqual(table.plainText, "T\n\n| a\\|b | n |\n| --- | --- |\n| y | 1 |\n| x | 2 |")
        let tree = try Visualization.parse(#"{"version":1,"type":"tree","title":"T","nodes":[{"label":"a","note":"n","children":[{"label":"b"}]}]}"#)
        XCTAssertEqual(tree.plainText, "T\n\n- a — n\n  - b")
        let line = try Visualization.parse(#"{"version":1,"type":"line_chart","title":"L","unit":"s","labels":["a","b"],"series":[{"name":"x","values":[1,null]}]}"#)
        XCTAssertEqual(line.plainText, "L\n\n| Label (s) | x |\n| --- | --- |\n| a | 1 |\n| b |  |")
        let checklist = try Visualization.parse(#"{"version":1,"type":"checklist","title":"C","summary":"S","items":[{"label":"a","status":"done"},{"label":"b","note":"n"}]}"#)
        XCTAssertEqual(checklist.plainText, "C\nS\n\n1 of 2 done\n[x] a\n[ ] b — n")
        XCTAssertEqual(try Visualization.parse(bar).plainText, "Build time\nengine dominates\n\nengine: 42 s\ntui: 10.5 s\ncore: 3 s")
    }

    // MARK: - Guide

    func testGuideCoversEveryTypeWithASampleThatRenders() throws {
        XCTAssertEqual(VisualizationGuide.entries.map(\.type), Visualization.supportedTypes)
        for entry in VisualizationGuide.entries {
            XCTAssertFalse(entry.prompt.isEmpty)
            XCTAssertEqual(try Visualization.parse(entry.sample).type, entry.type)
            guard case .visualization(let block) = MarkdownBlock.parse(entry.sampleMarkdown, visualizations: true).first,
                  case .rendered = block.state else { return XCTFail(entry.type) }
            XCTAssertLessThan(entry.sample.utf8.count, Visualization.maxPayloadBytes)
        }
    }

    func testStoredLevelOneMessagesRenderUnchanged() throws {
        // A reply saved before level 2 renders exactly as it did, and its
        // source fence is what copying reproduces.
        let stored = "Before\n\n```rustcode-viz\n\(bar)\n```\n\n```rustcode-viz\n{\"version\":1,\"type\":\"pie\"}\n```"
        let blocks = MarkdownBlock.parse(stored, visualizations: true)
        guard case .visualization(let first) = blocks[1], case .visualization(let second) = blocks[2] else {
            return XCTFail("\(blocks)")
        }
        XCTAssertEqual(first.state, .rendered(try Visualization.parse(bar)))
        XCTAssertEqual(first.source, "```rustcode-viz\n\(bar)\n```")
        XCTAssertEqual(second.state, .invalid("unsupported type \"pie\""))
    }
}
