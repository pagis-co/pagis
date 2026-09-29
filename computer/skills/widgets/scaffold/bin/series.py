#!/usr/bin/env python3
"""`chart__series`: the tool the widget alone calls, through
`tools/call`. It prints the widget's data half and nothing else, and
the model never hears of the call.

An app-only tool renders no widget, so it prints one plain JSON value.
The page reads it back from the text of the tool result."""

import json

import readings


def main():
    bars = readings.window(readings.arguments().get("days", readings.DEFAULT_DAYS))
    print(json.dumps(readings.chart(bars)))


main()
