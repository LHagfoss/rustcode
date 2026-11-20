import { describe, expect, test } from "bun:test";
import {
  failureHarness,
  failureStatus,
  networkFailure,
  readableTurnError,
  turnErrorAdvice,
} from "./turnError";

describe("a failed turn in words", () => {
  test("keeps Antigravity JSON-RPC diagnostics as supplied", () => {
    const error = 'Antigravity ACP error (-32603): Request failed — {"status":503,"message":"Unavailable"}';
    expect(readableTurnError(error, 97)).toBe(error);
  });

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

describe("a turn that lost the network", () => {
  test("reads the words each harness uses when no answer came back", () => {
    for (const text of [
      // Claude Code, whose own wording names nothing else.
      "Claude Code: API Error: Connection error",
      "Claude Code: Can't reach the API server. Check your internet connection and try again.",
      "Claude Code: Unable to reach the API server",
      // Codex: the stream ends before the answer does.
      "Codex: stream disconnected before completion: error sending request for url (https://chatgpt.com/backend-api/codex/responses)",
      "Codex: failed to connect to the model provider",
      // Pi, on the webview's own fetch.
      "Pi: TypeError: fetch failed",
      "Pi: fetch failed",
      // Node-style and proxy wordings that reach a turn as text.
      "Codex: connect ECONNREFUSED 127.0.0.1:443",
      "Codex: getaddrinfo ENOTFOUND api.openai.com",
      "Codex: socket hang up",
      "Claude Code: upstream connect error",
      "Codex: request failed: network error",
    ]) {
      expect(networkFailure(text)).toBe(true);
    }
  });

  test("a provider that answered is not the network", () => {
    for (const text of [
      undefined,
      "",
      // Reached, and refused on purpose.
      'Codex: {"detail":"Bad Request"}',
      'Claude Code: API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}',
      'Claude Code: API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"}}',
      "Codex: 503 Service Unavailable",
      'Claude Code: API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}',
      // Not network at all.
      "You've hit your 5-hour limit · resets 2:50pm (Europe/Oslo)",
      "Claude Code: your credit balance is too low",
      "Codex: the tool failed to connect to the database",
    ]) {
      expect(networkFailure(text)).toBe(false);
    }
  });
});

describe("the HTTP status a failure names", () => {
  test("is read from a body, a reason phrase or the text before it", () => {
    expect(failureStatus('Codex: {"detail":"Bad Request"}')).toBe(400);
    expect(failureStatus('Claude Code: API Error: 529 {"error":{"message":"Overloaded"}}')).toBe(529);
    expect(failureStatus("Codex: 503 Service Unavailable")).toBe(503);
    expect(failureStatus("Codex: the run finished")).toBeUndefined();
  });
});

describe("turnErrorAdvice", () => {
  test("a refused login offers Sign in, then Try again", () => {
    expect(turnErrorAdvice("Codex: OpenAI token request failed (HTTP 400)")).toEqual({
      signedOut: true,
      fixes: ["sign_in", "retry"],
    });
    expect(
      turnErrorAdvice('Claude Code: API Error: 401 {"error":{"message":"invalid x-api-key"}}').signedOut,
    ).toBe(true);
    expect(turnErrorAdvice("Gemini CLI: Please run /login to continue").fixes[0]).toBe("sign_in");
  });

  test("a full conversation refused offers Compact, then Try again", () => {
    expect(turnErrorAdvice('Codex: {"detail":"Bad Request"}', 96)).toEqual({
      signedOut: false,
      fixes: ["compact", "retry"],
    });
  });

  test("a bad request with room left offers nothing: the same request fails again", () => {
    expect(turnErrorAdvice('Codex: {"detail":"Bad Request"}', 30).fixes).toEqual([]);
  });

  test("no network, a server error and a rate limit offer Try again alone", () => {
    expect(turnErrorAdvice("Claude Code: Can't reach the API server").fixes).toEqual(["retry"]);
    expect(turnErrorAdvice("API Error: 529 Overloaded").fixes).toEqual(["retry"]);
    expect(turnErrorAdvice("HTTP 429 Too Many Requests").fixes).toEqual(["retry"]);
  });

  test("a server error that mentions a token is not a sign-in", () => {
    expect(turnErrorAdvice("HTTP 503: token service unavailable, unauthorized upstream").signedOut).toBe(
      false,
    );
  });

  test("nothing to read offers nothing", () => {
    expect(turnErrorAdvice(undefined)).toEqual({ signedOut: false, fixes: [] });
  });
});

describe("failureHarness", () => {
  test("reads the harness a failure names first", () => {
    expect(failureHarness("Claude Code: API Error: 401")).toBe("Claude Code");
    expect(failureHarness("fetch failed")).toBeUndefined();
  });
});
