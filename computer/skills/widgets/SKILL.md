---
name: widgets
description: Ship a widget in a Software Package, so a tool result draws a chart or a form in the conversation instead of text.
---

# Widgets

A widget is an HTML page your Software Package ships. When a tool of
that package renders a widget, the reader sees the page in the
conversation, and you see the text you wrote beside it.

Pagis is an MCP Apps host, so a package you write for Pagis also runs
in another MCP Apps host.

Read this Skill before you write the package. The contract below is
what the publish validation applies.

## When to ship a widget

Ship one when the reader must **see** a shape: a chart, a table, a
board, a small form. Do not ship one to say a sentence. A widget costs
a page, a schema and a review; a sentence costs nothing.

## The scaffold

The worked example is in your Computer:

```bash
cp -r /opt/pagis/skills/widgets/scaffold ~/software/readings
```

It holds one widget and the three shapes a tool has:

| file | what it is |
| --- | --- |
| `pagis-software.toml` | the manifest, with one `[[widget]]` |
| `widgets/chart.html` | the page: style, script and protocol in one file |
| `schemas/chart.json` | the schema of the data the page receives |
| `bin/show.py` | the tool the model calls, which renders the widget |
| `bin/series.py` | the tool the widget alone calls, to redraw itself |
| `bin/ask.py` | the tool that renders the widget and waits for an answer |

Rename the package in `pagis-software.toml`, put your own data behind
the tools, then `software_publish` the working copy.

## The manifest

```toml
[[tool]]
name = "show"
description = "Draw the last days of readings as a bar chart."
entry = "bin/show.py"
schema = "schemas/show.json"
widget = "chart"               # render into the widget of that name
visibility = ["model", "app"]  # the default; ["app"] hides it from you
awaits_input = false           # true parks the Run until the page answers

[[widget]]
name = "chart"
html = "widgets/chart.html"       # an HTML5 document in the package
schema = "schemas/chart.json"     # the JSON Schema of the data
csp = { connect = [], resource = [] }   # https origins, empty by default
```

The URI is derived: `ui://<package>/<widget>`. You do not write it.

## The result split

A tool that renders a widget prints one JSON object with two keys:

```json
{
  "content": "A chart of 7 readings. The highest is Day 13 at 38.",
  "structuredContent": { "title": "The last 7 readings", "bars": [] }
}
```

- `structuredContent` reaches the page alone. The daemon checks it
  against the widget schema. Data that fails the schema draws nothing,
  and you read the schema error instead.
- `content` reaches you alone, and it is your projection of the page.
  Write it as the whole answer: a reader on a surface that draws no
  widget reads this line and nothing else.

An app-only tool renders no widget, so it prints one plain JSON value.
The page reads that value back from the text of the tool result.

## The page

The daemon serves the page from the Version tree. The host loads it in a
sandboxed frame at an opaque origin of its own, so the page reaches
neither the conversation around it nor another widget.

**The page has no persistent storage.** `localStorage`,
`sessionStorage`, IndexedDB and cookies are not available at an opaque
origin, and the browser refuses each access. Do not write a page that
keeps state in the browser. The page gets its data from the tool result, and
it reads data again with `tools/call`.

The page speaks JSON-RPC with its parent over `postMessage`:

| direction | method | what it does |
| --- | --- | --- |
| out | `ui/initialize` | asks for the theme and the container width |
| out | `ui/notifications/initialized` | nothing reaches the page before it |
| in | `ui/notifications/tool-input` | the arguments of the tool call |
| in | `ui/notifications/tool-result` | the `structuredContent` |
| out | `ui/notifications/size-changed` | the height the host gives the frame |
| out | `tools/call` | one app-visible tool of the same package |
| out | `ui/message` | the one answer, when the tool awaits input |
| in | `ui/resource-teardown` | the Run ended |

Every other method is refused. `ui/open-link` and
`ui/update-model-context` are not available.

`tools/call` names the tool without the package prefix, and the daemon
qualifies it. The daemon refuses a tool of another package, a tool that
is not app-visible, and a tool that needs an approval: a page must
never park the Run that draws it.

## The two ways the page reaches you

**It does not, by default.** A widget that does not await input has no
path to the model. The reader can press its buttons, and you hear
nothing.

**`awaits_input = true`.** The Run parks on a `widget` Request beside
the block, and the page answers it once:

```js
request('ui/message', {
  role: 'user',
  content: { type: 'text', text: 'Look into Day 13.' },
  value: { day: 'Day 13' },
})
```

The text is required and is at most 4096 characters; `value` is any
JSON and is at most 64 KB. The answer reaches you inside the untrusted
envelope, because a reader wrote it. A second answer is refused: the
Request is one-shot.

## The wall

The daemon serves the page under this policy, built from your `csp`
lists:

```
default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self'
'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; media-src
'self' data:; connect-src 'none'; frame-src 'none'; base-uri 'none';
form-action 'none'
```

- Put the style and the script **inside** the page. The page loads no
  file of its own.
- `connect-src 'none'` is the wall. A page with no declared origin
  reaches no network at all. Read your data through a tool, never with
  `fetch`.
- A declared origin must be an `https` origin, such as
  `https://cdn.example.com`. `connect` opens requests; `resource`
  opens scripts, styles, images, fonts and media. Declare nothing you
  do not need: every origin you open is an origin your data can leave
  through.

## The caps

| cap | value |
| --- | --- |
| the page | 1 MB |
| `structuredContent` | 256 KB |
| widgets per tool result | 1 |
| live widgets in one conversation | 20 |

Older widgets on the same screen show their projection until the reader
scrolls back to them.

## What the publish refuses

- a `html` or a `schema` file that is not in the package;
- a page over 1 MB;
- a `csp` entry that is not an `https` origin;
- a tool whose `widget` names no declared widget;
- `awaits_input` on a tool that renders no widget;
- an empty `visibility`.

## The order of work

1. Fork the scaffold and rename the package.
2. Write the widget schema first. It is the contract between the tool
   and the page.
3. Make the tool print the two halves, and read them with
   `python3 -c` before you publish.
4. `software_publish` the package, then call the tool once and look at
   the result the daemon gives back.
