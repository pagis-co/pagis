# 0016: A Software Package is a tag-addressed Version

Status: accepted.

## Context

The software factory lets an Agent publish a Software Package from its
Computer so that any Agent in the Workspace can call its tools. A Software
Package belongs to one Workspace: the Org installs Plugins, and each Workspace
keeps its own Software List. A published Version must be immutable, because a
Capability Snapshot records it, and the daemon must make a patch between two
Versions for a Contribution. The daemon already drives git for memory, and
every operation the store needs is one library call with no working tree.

## Decision

### A Version is a git tag in a daemon-owned bare repository

Each Software Package has one bare git repository that only the daemon writes,
at `<data root>/software/<workspace_id>/<package>.git`. A publish commits the
package tree with the previous Version as parent and marks it with an
annotated tag. The daemon names the tag `v1`, `v2` and so on; the manifest has
no version field, and the author cannot choose or reuse one. The tag is the
source version of the Capability Manifest. A publish whose tree equals the
previous Version is refused. The manifest of each Version is also in the
database, so Run start and search never open git.

A Run resolves every package to its latest Version at start and keeps it to
the end; a publish during a Run changes no running snapshot. There is no
install step and no pin, so a fix published once reaches every caller at its
next Run. A Version is never rewritten. The daemon runs no git remote, and the
repositories are not a place to collaborate.

### A Contribution is a record and a message

A Contribution is a daemon-owned record: the origin package, the base
Version, the fork Version, a patch the daemon computes between the two trees,
a summary, and a status of open, merged or declined. The daemon delivers it to
the author Agent as a message from the forker in their direct Channel, with
the patch inline under a size cap. The author merges it in its own working
copy, publishes a new Version and closes the record, and the close posts the
outcome to the forker.

There is no review interface, no comment thread, no automatic merge and no new
trigger kind; the desk shows Contributions read-only. The daemon never applies
a patch to an author's package, so only the author's publish changes it. A
Contribution with an old base is delivered as it is, and conflicts are the
author's work.

### A Software Package ships an MCP Apps widget, and Pagis is the host

The field converges on MCP Apps for generated UI, and Pagis is an MCP Apps
host for this subset:

- **Declaration.** The manifest has widget entries: a name, an HTML file in
  the tree, a JSON Schema file for the widget's data, and a content-security
  list of connect and resource domains, empty by default. A tool entry names a
  widget to render into and can hide the tool from the model. The URI derives
  from the package and widget name. Install checks that the files exist.
- **One render path.** Only a tool result renders a widget. Its structured
  content goes to the widget alone, validated against the widget schema; its
  text content goes to the model alone. The daemon makes the block. Content
  that fails the schema makes no block and returns a schema error. `add_block`
  cannot render a widget.
- **The block.** A `widget` block carries the package, the version, the widget
  name, the tool call id and an optional Request id, and no HTML. The UI
  fetches the resource through the daemon by package, version and URI,
  immutable for each Version, so an old message renders its own Version. The
  projection is the result's text content, marked untrusted wherever it
  reaches a model.
- **Two interactions.** A view calls its own package's app-only tools through
  the broker, where the snapshot and effect class decide the approval and the
  model never hears of it. A tool can declare that its widget awaits input:
  the Run parks on a `widget` Request, and the view answers once with text and
  an optional JSON value, which the Run receives inside the untrusted envelope.
  A view that awaits no input has no path to the model.
- **Render model.** The specification's sandbox proxy on a second origin that
  the daemon serves. The proxy loads each widget page through `srcdoc` into an
  inner frame with `sandbox="allow-scripts"` and without
  `allow-same-origin`, so each page has its own opaque origin and no
  persistent storage. The daemon runs widget HTML only in the proxy; its route
  at the Product App origin returns the source as inert data
  (`application/octet-stream`, an attachment, `nosniff`, a `sandbox` CSP). The
  content-security policy comes from the declared lists and denies everything
  else. Host context is the theme and the container size. Tool input and result
  notifications go to the view, a resource read stays inside the package's
  version tree, and a teardown fires when the Run ends.
- **Caps.** 1 MB of HTML for each widget, 256 KB of structured content, one
  widget for each tool result, and at most 20 live widget blocks in one
  conversation view; older ones show their projection until scrolled to.
- **Teaching.** A first-party skill in the Computer image, listed for every
  Agent, carries the contract, a scaffold package with one chart widget, and
  the install checks.

A package built for Pagis also runs in any MCP Apps host.

## Consequences

- The package format, the Version model, the audit trail and the approval gate
  do not depend on widgets.
- `widget` is a Request kind.
- The deletion or retirement of a package and its repository is not decided.
