import Foundation
import Testing
@testable import RustCodeRemote

@Suite struct ImageUploadTests {
    @Test func chunkWireFormatIsSessionScopedAndBelowFrameLimit() throws {
        let session = SessionRef(sessionID: "session", registrationEpoch: 4)
        let data = Data(repeating: 7, count: PromptImage.chunkBytes)
        let chunk = ImageUploadChunk(attachmentID: "image-1", mimeType: "image/png", offset: 0,
                                    totalBytes: UInt64(data.count), dataBase64: data.base64EncodedString())
        let encoded = try WireCodec().encode(.uploadImage(session, chunk), requestID: RequestID("chunk-1"))
        #expect(encoded.utf8.count < 256 * 1024)
        let request = try JSONDecoder().decode(WireRequest.self, from: Data(encoded.utf8))
        #expect(request.sessionID == session.sessionID)
        #expect(request.registrationEpoch == 4)
        #expect(request.operation == .uploadImage(chunk))
    }

    @Test func imageReplyRequiresAppliedReceiptBeforeReferenceCanBeUsed() throws {
        func frame(_ receipt: String) -> String {
            #"{"kind":"response","protocol_version":1,"request_id":"image","receipt":"\#(receipt)","result":{"type":"image_uploaded","attachment_id":"photo","next_offset":3,"total_bytes":3,"complete":true,"prompt_reference":"![image](file:///image.png)"}}"#
        }
        #expect(try WireCodec().decode(frame("unknown")) == .reply(requestID: RequestID("image"), .receipt(.unknown)))
        #expect(try WireCodec().decode(frame("received")) == .reply(requestID: RequestID("image"), .receipt(.received)))
        guard case .reply(_, .imageUploaded(let progress)) = try WireCodec().decode(frame("applied")) else {
            Issue.record("Missing applied image progress"); return
        }
        #expect(progress.complete)
        #expect(progress.promptReference == "![image](file:///image.png)")
    }

    @Test func demoUploadSimulatesIdempotentProgressWithoutFilesystemWrites() async throws {
        let client = DemoRemoteClient()
        let credential = try await client.pair(.scanned("demo"), deviceName: "test")
        try await client.connect(using: credential)
        let ref = SessionRef(sessionID: DemoRemoteClient.idleSessionID, registrationEpoch: 1)
        let chunk = ImageUploadChunk(attachmentID: "photo", mimeType: "image/png", offset: 0, totalBytes: 3,
                                    dataBase64: Data([1, 2, 3]).base64EncodedString())
        let id = RequestID()
        let result = try await client.uploadImage(chunk, in: ref, requestID: id)
        #expect(result.complete)
        #expect(try await client.uploadImage(chunk, in: ref, requestID: id) == result)
        var changed = chunk
        changed.dataBase64 = Data([4, 5, 6]).base64EncodedString()
        await #expect(throws: RemoteError.rejected(.unsupportedOperation)) {
            try await client.uploadImage(changed, in: ref, requestID: id)
        }
        await client.disconnect()
    }
}

@Suite struct ImageAttachmentPresentationTests {
    let reference = "![image](file:///private/config/session/remote-images/" + String(repeating: "a", count: 64) + "/photo-1.png)"

    @Test func validatedUploadMarkersDisplayWithoutPrivatePaths() {
        #expect(ImageAttachmentPresentation.displayText("Describe\n\n" + reference) == "Describe\n\nImage attachment")
        #expect(ImageAttachmentPresentation.displayText("![image](file:///demo/photo.png)") == "Image attachment")
    }

    @Test func literalExamplesAndUnrelatedFilesRemainUnchanged() {
        for raw in ["Example: " + reference, "```markdown\n" + reference + "\n```", "    " + reference,
                    "![image](file:///Users/me/photo.png)", "![image](https://example.com/photo.png)"] {
            #expect(ImageAttachmentPresentation.displayText(raw) == raw)
        }
    }
}
