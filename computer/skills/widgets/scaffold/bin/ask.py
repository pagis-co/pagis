#!/usr/bin/env python3
"""`chart__ask`: the tool that parks its Run on a `widget` Request.

The manifest sets `awaits_input = true`, so the daemon mints the
Request beside the block and the Run waits. The page asks the question
this tool puts in the data half, and answers once with `ui/message`."""

import readings

DEFAULT_QUESTION = "Which day should I look into?"


def main():
    given = readings.arguments()
    bars = readings.window(given.get("days", readings.DEFAULT_DAYS))
    question = given.get("question") or DEFAULT_QUESTION
    top = readings.highest(bars)
    readings.emit(
        f"A chart of {len(bars)} readings, the highest {top['label']} at "
        f"{top['value']}. The reader is asked: {question}",
        readings.chart(bars, question),
    )


main()
