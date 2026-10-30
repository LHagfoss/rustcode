import XCTest
@testable import RustCodeKit

final class GitChangesTests: XCTestCase {
    func testParsesModifiedAddedDeletedRenamedAndBinaryFiles() {
        let diff = """
        diff --git a/src/lib.rs b/src/lib.rs
        index 1111111..2222222 100644
        --- a/src/lib.rs
        +++ b/src/lib.rs
        @@ -10,3 +10,4 @@ fn main() {
         let a = 1;
        -let b = 2;
        +let b = 3;
        +let c = 4;
         let d = 5;
        @@ -40,2 +41,2 @@
         x
        -y
        +z
        diff --git a/new file.txt b/new file.txt
        new file mode 100644
        --- /dev/null
        +++ b/new file.txt
        @@ -0,0 +1,2 @@
        +hello
        +world
        \\ No newline at end of file
        diff --git a/gone.txt b/gone.txt
        deleted file mode 100644
        --- a/gone.txt
        +++ /dev/null
        @@ -1 +0,0 @@
        -bye
        diff --git a/old.md b/docs/new.md
        similarity index 100%
        rename from old.md
        rename to docs/new.md
        diff --git a/logo.png b/logo.png
        index 3333333..4444444 100644
        Binary files a/logo.png and b/logo.png differ

        """
        let files = UnifiedDiff.parse(diff)
        XCTAssertEqual(files.map(\.path), ["src/lib.rs", "new file.txt", "gone.txt", "docs/new.md", "logo.png"])
        XCTAssertEqual(files.map(\.status), [.modified, .added, .deleted, .renamed, .modified])

        let lib = files[0]
        XCTAssertEqual(lib.added, 3)
        XCTAssertEqual(lib.removed, 2)
        XCTAssertEqual(lib.rows.first, .gap(9))
        XCTAssertEqual(lib.rows[2], .line(DiffLine(kind: .removed, text: "let b = 2;", oldNumber: 11)))
        XCTAssertEqual(lib.rows[3], .line(DiffLine(kind: .added, text: "let b = 3;", newNumber: 11)))
        XCTAssertTrue(lib.rows.contains(.gap(27)), "the lines between hunks are counted")

        XCTAssertEqual(files[1].added, 2)
        XCTAssertEqual(files[3].oldPath, "old.md")
        XCTAssertTrue(files[3].rows.isEmpty)
        XCTAssertTrue(files[4].isBinary)
    }

    func testALongFileKeepsItsHeadAndCountsTheRest() {
        let body = (1...10).map { "+line \($0)" }.joined(separator: "\n")
        let diff = "diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n@@ -0,0 +1,10 @@\n\(body)\n"
        let file = UnifiedDiff.parse(diff, maxLinesPerFile: 4)[0]
        XCTAssertEqual(file.rows.count, 4)
        XCTAssertEqual(file.omittedLines, 6)
        XCTAssertEqual(file.added, 10, "counts cover the whole file")
    }

    // MARK: - Against a real repository

    private var repo: URL!

    override func setUpWithError() throws {
        repo = FileManager.default.temporaryDirectory.appendingPathComponent("rustcode-changes-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: repo, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: repo)
    }

    @discardableResult
    private func git(_ arguments: String...) throws -> String {
        try GitChangesLoader().run(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"] + arguments,
                                   in: repo.path)
    }

    private func write(_ name: String, _ text: String) throws {
        try text.write(to: repo.appendingPathComponent(name), atomically: true, encoding: .utf8)
    }

    func testAFolderOutsideGitSaysSo() async {
        do {
            _ = try await GitChangesLoader().load(folder: repo.path, scope: .workingTree)
            XCTFail("expected an error")
        } catch {
            XCTAssertEqual(error as? ChangesError, .notARepository)
        }
    }

    func testWorkingTreeAndBranchScopes() async throws {
        try git("init", "-q", "-b", "main")
        try write("a.txt", "one\n")
        try git("add", ".")
        try git("commit", "-qm", "first")
        try git("switch", "-qc", "feature")
        try write("a.txt", "one\ntwo\n")
        try git("commit", "-qam", "second")
        // Not committed: a change and an untracked file.
        try write("a.txt", "one\ntwo\nthree\n")
        try write("new.txt", "fresh\n")

        let loader = GitChangesLoader()
        let tree = try await loader.load(folder: repo.path, scope: .workingTree)
        XCTAssertEqual(tree.branch, "feature")
        XCTAssertEqual(tree.files.map(\.path), ["a.txt", "new.txt"])
        XCTAssertEqual(tree.files.map(\.added), [1, 1])
        XCTAssertEqual(tree.files[1].status, .untracked)

        let branch = try await loader.load(folder: repo.path, scope: .branch(base: "main"))
        XCTAssertEqual(branch.files.first?.added, 2, "the branch's commit and the uncommitted line")
        XCTAssertNotEqual(branch.baseCommit, tree.baseCommit)
        XCTAssertEqual(loader.defaultBase(folder: repo.path), "main")
        XCTAssertEqual(Set(loader.branches(folder: repo.path)), ["main", "feature"])

        do {
            _ = try await loader.load(folder: repo.path, scope: .branch(base: "nope"))
            XCTFail("expected an error")
        } catch {
            XCTAssertEqual(error as? ChangesError, .unknownBase("nope"))
        }
        // Read-only: nothing was staged.
        XCTAssertEqual(try git("diff", "--cached", "--name-only"), "")
    }

    func testARepositoryWithoutCommitsComparesWithNothing() async throws {
        try git("init", "-q")
        try write("a.txt", "x\n")
        try git("add", "a.txt")
        let snapshot = try await GitChangesLoader().load(folder: repo.path, scope: .workingTree)
        XCTAssertEqual(snapshot.files.map(\.status), [.added])
        XCTAssertNil(snapshot.baseCommit)
    }
}
