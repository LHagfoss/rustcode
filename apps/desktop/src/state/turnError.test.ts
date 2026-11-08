import { describe, expect, test } from "bun:test";
import { readableTurnError } from "./turnError";

describe("a failed turn in words", () => {
  test("a JSON body is read for its message, with the harness and the HTTP status", () => {
    expect(readableTurnError('Codex: {"detail":"Bad Request"}')).toBe(
      "Codex rejected the request (HTTP 400 Bad Request).",
    );
    expect(
      readableTurnError(
        'Claude Code: API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"}}',
      ),
    ).toBe("Claude Code rejected the request (HTTP 429 Too Many Requests): Rate limited.");
    expect(readableTurnError('Codex: {"message":"model not found","status":404}')).toBe(
      "Codex rejected the request (HTTP 404 Not Found): model not found.",
    );
    expect(readableTurnError('{"error":"invalid_api_key"}')).toBe(
      "The provider rejected the request: invalid_api_key.",
    );
  });

  test("a full conversation says to compact or start a new thread", () => {
    expect(readableTurnError('Codex: {"detail":"Bad Request"}', 97)).toBe(
      "Codex rejected the request (HTTP 400 Bad Request). The conversation is 97% full — compact it or start a new thread.",
    );
    // Below the mark, or a provider that failed on its side, says nothing about the size.
    expect(readableTurnError('Codex: {"detail":"Bad Request"}', 89)).toBe(
      "Codex rejected the request (HTTP 400 Bad Request).",
    );
    expect(readableTurnError('Codex: {"detail":"Service Unavailable"}', 97)).toBe(
      "Codex could not answer the request (HTTP 503 Service Unavailable).",
    );
  });

  test("text that is already words stays as it is", () => {
    expect(readableTurnError("Codex exited during the turn", 97)).toBe("Codex exited during the turn");
    expect(readableTurnError("The engine stopped during this turn.")).toBe(
      "The engine stopped during this turn.",
    );
    expect(readableTurnError("Codex: {not json}")).toBe("Codex: {not json}");
    // A status with no body is kept, and a full conversation still says why it was refused.
    expect(readableTurnError("Codex: unexpected status 400 Bad Request", 95)).toBe(
      "Codex: unexpected status 400 Bad Request. The conversation is 95% full — compact it or start a new thread.",
    );
  });

  test("HTML bodies remain text and very long errors stay compact", () => {
    const html = "Codex: <!DOCTYPE html><html><body>400 Bad Request</body></html>";
    expect(readableTurnError(html, 97)).toBe(html);
    const htmlWithJson = 'Codex: <html><body>{"detail":"Bad Request"}</body></html>';
    expect(readableTurnError(htmlWithJson, 97)).toBe(htmlWithJson);

    const raw = `Codex: {"detail":"${"x".repeat(2_000)}"}`;
    const readable = readableTurnError(raw);
    expect(readable.startsWith("Codex rejected the request:")).toBe(true);
    expect(readable.length).toBeLessThanOrEqual(500);
    expect(readable.endsWith("….")).toBe(true);
    const withHint = readableTurnError(raw, 97);
    expect(withHint).toContain("The conversation is 97% full");
    expect(withHint.length).toBeLessThanOrEqual(500);

    const plain = `Codex: ${"y".repeat(2_000)}`;
    expect(readableTurnError(plain).length).toBeLessThanOrEqual(500);
  });
});
