#!/usr/bin/env python3
"""Offline JSONL adapter for a preloaded local laya-mlx checkpoint."""

from __future__ import annotations

import argparse
import json
import math
import platform
import sys
import time
from pathlib import Path
from typing import Any

PROTOCOL_VERSION = 1
MAX_LINE_BYTES = 16 * 1024
MAX_REQUEST_ID_BYTES = 256
PINNED_LAYA_MLX_VERSION = "0.2.0"
SUPPORTED_KINDS = ("shell_policy", "repetition")
SUPPORTED_LABELS = {
    "read_only",
    "novel_evidence",
    "confirmatory_evidence",
    "no_new_information",
    "unknown",
}


def _write_json(message: dict[str, Any]) -> None:
    encoded = json.dumps(message, separators=(",", ":"), allow_nan=False).encode("utf-8")
    if len(encoded) + 1 > MAX_LINE_BYTES:
        raise ValueError("sidecar message exceeds protocol line limit")
    sys.stdout.buffer.write(encoded + b"\n")
    sys.stdout.buffer.flush()


def _error_response(request_id: str, category: str) -> dict[str, Any]:
    return {
        "protocol": PROTOCOL_VERSION,
        "id": request_id,
        "ok": False,
        "error": {"category": category},
    }


def _validate_request(request: Any) -> tuple[str, str, dict[str, Any]]:
    if not isinstance(request, dict) or request.get("protocol") != PROTOCOL_VERSION:
        raise ValueError("invalid protocol")
    request_id = request.get("id")
    kind = request.get("kind")
    input_data = request.get("input")
    deadline_ms = request.get("deadline_ms")
    if (
        not isinstance(request_id, str)
        or not request_id
        or len(request_id.encode("utf-8")) > MAX_REQUEST_ID_BYTES
        or kind not in SUPPORTED_KINDS
        or not isinstance(input_data, dict)
        or isinstance(deadline_ms, bool)
        or not isinstance(deadline_ms, int)
        or deadline_ms < 0
    ):
        raise ValueError("invalid request fields")
    if len(json.dumps(input_data, separators=(",", ":")).encode("utf-8")) > MAX_LINE_BYTES // 2:
        raise ValueError("request input is too large")
    return request_id, kind, input_data


def _questions(kind: str) -> dict[str, Any]:
    if kind == "shell_policy":
        return {
            "decision": {
                "type": "choice",
                "instructions": "Classify the requested operation conservatively.",
                "criteria": ["read_only", "unknown"],
            }
        }
    return {
        "decision": {
            "type": "choice",
            "instructions": "Classify whether the bounded recovery has new evidence.",
            "criteria": [
                "novel_evidence",
                "confirmatory_evidence",
                "no_new_information",
                "unknown",
            ],
        }
    }


def _answer_value(result: Any) -> Any:
    if not isinstance(result, dict):
        raise ValueError("model result is not an object")
    answers = result.get("answers")
    if not isinstance(answers, dict) or "decision" not in answers:
        raise ValueError("model result has no decision answer")
    return answers["decision"]


def _normalize_decision(result: Any, kind: str, input_data: dict[str, Any]) -> dict[str, Any]:
    answer = _answer_value(result)
    if isinstance(answer, dict):
        label = answer.get("choice", answer.get("label"))
        confidence = answer.get("confidence", answer.get("probability"))
    else:
        label = answer
        confidence = result.get("confidence") if isinstance(result, dict) else None
    if not isinstance(label, str) or label not in SUPPORTED_LABELS:
        raise ValueError("model returned an unsupported label")
    if isinstance(confidence, bool) or not isinstance(confidence, (int, float)):
        raise ValueError("model returned no confidence")
    confidence = float(confidence)
    if not math.isfinite(confidence) or not 0.0 <= confidence <= 1.0:
        raise ValueError("model returned invalid confidence")

    effects = input_data.get("candidate_effects")
    if not isinstance(effects, list) or not all(isinstance(effect, str) for effect in effects):
        effects = ["read_only"] if kind == "repetition" else ["unknown"]
    return {
        "label": label,
        "confidence": confidence,
        "effects": effects[:16],
        "rationale_code": f"laya_{kind}",
    }


def _load_agent(model_path: Path) -> Any:
    if not model_path.exists() or not (model_path.is_file() or model_path.is_dir()):
        raise RuntimeError("configured model path is not readable")
    try:
        import laya_mlx as laya
    except Exception as exc:  # pragma: no cover - depends on the local MLX install
        raise RuntimeError("could not import pinned laya-mlx") from exc
    if getattr(laya, "__version__", PINNED_LAYA_MLX_VERSION) != PINNED_LAYA_MLX_VERSION:
        raise RuntimeError("installed laya-mlx version is not the pinned version")
    # Passing a local path is intentional: this adapter never accepts a Hub ID
    # and never downloads a checkpoint during a RustCode turn.
    return laya.load(str(model_path))


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="RustCode's offline Laya JSONL sidecar")
    parser.add_argument("--model", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    if sys.version_info < (3, 11):
        print("Laya sidecar requires Python >= 3.11", file=sys.stderr)
        return 2
    if sys.platform != "darwin" or platform.machine() != "arm64":
        print("Laya sidecar requires Apple Silicon macOS", file=sys.stderr)
        return 2

    args = _parse_args()
    try:
        agent = _load_agent(args.model)
    except Exception as exc:  # startup failures must be visible and nonzero
        print(f"Laya sidecar startup failed: {exc}", file=sys.stderr)
        return 2

    try:
        _write_json(
            {
                "protocol": PROTOCOL_VERSION,
                "backend": "laya-mlx",
                "model": args.model.name,
                "kinds": list(SUPPORTED_KINDS),
            }
        )
    except Exception as exc:
        print(f"Laya sidecar readiness failed: {exc}", file=sys.stderr)
        return 2

    for raw_line in sys.stdin.buffer:
        if len(raw_line) > MAX_LINE_BYTES or not raw_line.endswith(b"\n"):
            print("Laya sidecar rejected an oversized or unterminated request", file=sys.stderr)
            continue
        request = None
        try:
            request = json.loads(raw_line)
            request_id, kind, input_data = _validate_request(request)
        except Exception:
            print("Laya sidecar rejected an invalid request", file=sys.stderr)
            request_id = request.get("id") if isinstance(request, dict) else None
            if isinstance(request_id, str) and request_id:
                try:
                    _write_json(_error_response(request_id, "invalid_request"))
                except Exception as exc:
                    print(f"Laya sidecar response failed: {exc}", file=sys.stderr)
                    return 2
            continue

        started = time.monotonic()
        try:
            result = agent.predict(input_data, _questions(kind))
            decision = _normalize_decision(result, kind, input_data)
            response = {
                "protocol": PROTOCOL_VERSION,
                "id": request_id,
                "ok": True,
                "decision": decision,
                "latency_ms": round((time.monotonic() - started) * 1000),
            }
        except Exception as exc:  # diagnostics stay off the protocol stream
            print(f"Laya sidecar inference failed: {exc}", file=sys.stderr)
            response = _error_response(request_id, "model_error")
        try:
            _write_json(response)
        except Exception as exc:
            print(f"Laya sidecar response failed: {exc}", file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
