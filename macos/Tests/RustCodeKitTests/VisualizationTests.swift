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
        XCTAssertEqual(reason(#"{"version":1,"type":"table","columns":["a"],"rows":[],"sort_by":{"column":3}}"#),
                       "\"sort_by.column\" must be a shown column index")
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

    func testTreeNodesKeepNotesAndUniqueIds() throws {
        let body = #"{"version":1,"type":"tree","nodes":[{"label":"ui/","children":[{"label":"markdown.rs","note":"tables"},{"label":"modals/","children":[{"label":"theme.rs"}]}]}]}"#
        guard case .tree(let nodes, let hidden) = try Visualization.parse(body).content else { return XCTFail() }
        XCTAssertEqual(hidden, 0)
        XCTAssertEqual(nodes[0].children[0].note, "tables")
        XCTAssertEqual([nodes[0].id, nodes[0].children[0].id, nodes[0].children[1].id, nodes[0].children[1].children[0].id], [0, 1, 2, 3])
    }

    func testInitializeAdvertisesVisualizations() {
        let params = ACPRequests.initialize(appVersion: "1.0")
        XCTAssertEqual(params["clientCapabilities"]?["_meta"]?["rustcode"]?["visualizations"], .number(1))
    }
}
