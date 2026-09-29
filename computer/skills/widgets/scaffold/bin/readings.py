"""What the three tools share: the arguments on stdin, the readings on
disk, and the two halves of one widget result."""

import json
import sys
from pathlib import Path

DATA = Path(__file__).resolve().parent.parent / "data" / "readings.json"
DEFAULT_DAYS = 7
MAX_DAYS = 14


def arguments():
    """The argument object the daemon writes to stdin."""
    text = sys.stdin.read().strip()
    return json.loads(text) if text else {}


def window(days):
    """The last `days` readings, newest last."""
    if not isinstance(days, int) or isinstance(days, bool):
        days = DEFAULT_DAYS
    days = max(1, min(days, MAX_DAYS))
    return json.loads(DATA.read_text())[-days:]


def chart(bars, question=None):
    """The data half the widget reads. It must match
    `schemas/chart.json`, or the daemon draws nothing and the model
    reads the schema error instead."""
    data = {"title": f"The last {len(bars)} readings", "bars": bars}
    if question is not None:
        data["question"] = question
    return data


def highest(bars):
    return max(bars, key=lambda bar: bar["value"])


def emit(content, structured_content):
    """One widget result: `content` reaches the model alone, and
    `structuredContent` reaches the widget alone."""
    print(json.dumps({"content": content, "structuredContent": structured_content}))
