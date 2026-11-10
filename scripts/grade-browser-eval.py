#!/usr/bin/env python3
"""Grade a browser evaluation run from its independent answer and page evidence."""
import json
import pathlib
import sys


def decode_page(raw):
    # WebKit's callback returns the JSON representation of the JS string.
    try:
        value = raw
        for _ in range(2):
            if isinstance(value, str):
                value = json.loads(value)
        return value if isinstance(value, dict) else {}
    except (TypeError, ValueError):
        return {}


def grade(root):
    checks = {
        "01-overview": ["林岚", "09:30", "17:30"],
        "02-navigation": ["周宁", "2027-03-18", "验收中"],
        "03-form": ["HX-2048"],
        "04-frame": ["正常", "2026-11-03", "02:00"],
        "05-cross-origin": ["DIAG-314"],
    }
    summary = []
    for task, terms in checks.items():
        report_path = root / f"{task}.json"
        if not report_path.exists():
            summary.append({"task": task, "pass": False, "reason": "missing report"})
            continue
        report = json.loads(report_path.read_text())
        answer = "\n".join(block.get("text", "") for block in report.get("answer") or [] if block.get("type") == "text")
        page = decode_page(report.get("page"))
        missing = [term for term in terms if term not in answer]
        if task == "01-overview" and not ("周一" in answer and "周五" in answer):
            missing.append("周一至周五")
        reasons = []
        if report.get("status") != "complete":
            reasons.append(report.get("status", "missing status"))
        if missing:
            reasons.append("answer missing: " + ", ".join(missing))
        if task == "03-form":
            try:
                submission = json.loads(page.get("submission") or "null")
            except ValueError:
                submission = None
            if submission != {"name": "陈星", "kind": "equipment", "reason": "测试工位显示器"}:
                reasons.append("page does not confirm expected submission")
        if task == "05-cross-origin" and page.get("diagnosis") != "DIAG-314":
            reasons.append("page does not confirm diagnostic action")
        events_path = root / f"{task}.events.jsonl"
        events = [json.loads(line)["payload"].get("event", {}) for line in events_path.read_text().splitlines()] if events_path.exists() else []
        starts = [event for event in events if event.get("type") == "tool_execution_start"]
        errors = [event for event in events if event.get("type") == "tool_execution_end" and event.get("isError")]
        references = {}
        clicked = []
        for event in events:
            if event.get("type") == "tool_execution_end":
                for block in event.get("result", {}).get("content", []):
                    text = block.get("text", "")
                    if "\n" not in text:
                        continue
                    try:
                        result = json.loads(text.split("\n", 1)[1])
                    except ValueError:
                        continue
                    if "elements" in result:
                        references = {element["id"]: element.get("text", "") for element in result["elements"]}
            elif event.get("type") == "tool_execution_start" and event.get("args", {}).get("action") == "click":
                clicked.append(references.get(event["args"].get("element"), ""))
        if not starts or any(event.get("toolName") != "browser" for event in starts):
            reasons.append("expected browser-only tool calls")
        if task == "04-frame" and "启动诊断" in clicked:
            reasons.append("unrequested diagnostic action")
        summary.append({"task": task, "pass": not reasons, "reasons": reasons, "seconds": report.get("seconds"), "toolCalls": len(starts), "toolErrors": len(errors), "clicked": clicked, "session": report.get("session"), "answer": answer})
    return summary


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: grade-browser-eval.py RESULTS_DIR")
    results = grade(pathlib.Path(sys.argv[1]))
    print(json.dumps(results, ensure_ascii=False, indent=2))
    raise SystemExit(0 if all(result["pass"] for result in results) else 1)
