#!/usr/bin/env python3
"""`chart__show`: the tool the model calls. It renders the widget."""

import readings


def main():
    bars = readings.window(readings.arguments().get("days", readings.DEFAULT_DAYS))
    top = readings.highest(bars)
    readings.emit(
        f"A chart of {len(bars)} readings. "
        f"The highest is {top['label']} at {top['value']}.",
        readings.chart(bars),
    )


main()
