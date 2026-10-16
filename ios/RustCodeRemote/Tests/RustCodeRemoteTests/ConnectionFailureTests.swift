import Foundation
import Testing
@testable import RustCodeRemote

struct ConnectionFailureTests {
    private func failure(_ code: URLError.Code, url: String? = "ws://192.168.1.20:17879/remote") -> URLError {
        URLError(code, userInfo: url.flatMap(URL.init(string:)).map { [NSURLErrorFailingURLErrorKey: $0] } ?? [:])
    }

    @Test func refusedConnectionNamesTheAddressAndTheLikelyCause() {
        let text = ConnectionFailure.describe(failure(.cannotConnectToHost))
        #expect(text.hasPrefix("Nothing is listening at 192.168.1.20:17879."))
        #expect(text.contains("/remote"))
        #expect(text.hasSuffix("(URL error -1004)"))
    }

    @Test func eachCommonCauseHasItsOwnReason() {
        #expect(ConnectionFailure.describe(failure(.timedOut)).contains("same Wi-Fi"))
        #expect(ConnectionFailure.describe(failure(.cannotFindHost)).contains("Could not find 192.168.1.20:17879"))
        #expect(ConnectionFailure.describe(failure(.notConnectedToInternet)).contains("Local Network"))
    }

    @Test func missingAddressAndUnknownCodesStayReadable() {
        #expect(ConnectionFailure.describe(failure(.cannotConnectToHost, url: nil)).hasPrefix("Nothing is listening at the Mac."))
        let unknown = failure(.cannotParseResponse)
        #expect(ConnectionFailure.describe(unknown) == "\(unknown.localizedDescription) (URL error -1017)")
    }

    @Test func otherErrorsKeepTheirOwnDescription() {
        #expect(ConnectionFailure.describe(RemoteError.timeout) == "The host did not answer in time.")
    }
}
