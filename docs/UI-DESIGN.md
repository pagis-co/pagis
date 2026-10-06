# The Pagis interface

This document is the design system of `ui/`: the thesis, the surfaces,
and the tokens and primitives every screen renders through. It is the
reference a new screen is written against.

## An office, not a chat log

The product lets a person run their life and work with a staff of AI
agents. The interface must therefore show the staff. Every surface
answers one of three questions: who is working, what do they need from
me, and what did they do.

The identity is warm, quiet and confident. A well-run office. Not a
terminal, and not a consumer chat app.

## Faces first

Every Agent has an avatar, a chosen image or its initial on a
deterministic hue, and an Activity Ring: working, needs you, on a call,
or idle. The same face appears in the sidebar, the message row, the
computer tile, the run row and the profile, so one Agent reads as one
person everywhere.

One Agent has two states, each with its own words, and no word is in
both sets. The Activity says what the Agent does: Working, Needs you,
On a call or Idle. The Computer state says whether its Computer runs:
Awake, Asleep, Starting, Downloading or Needs attention. The screen
frame says who is in control of the screen, which is not work.
`src/components/stateWords.ts` holds every one of these words, and
every surface that shows a state reads it.

A ring calls, and a caption reports. A ring turns only for work the user
waits on, in the Channel that work happens in.

## The surfaces

Every destination, Channel, Thread, Run, Agent and settings section has
a URL, so a refresh keeps the place and any view is linkable.

- **Home** (`/`) — the needs-you queue first: approvals, questions,
  failed Runs, missed calls, each with an inline action. A failed Run or
  a missed call also has Dismiss, and it leaves the queue when the
  person opens it, calls back or dismisses it. Then the Report, then
  the work record of the day.
- **Sprites** (`/sprites`, `/sprites/<id>`) — the roster and a profile
  for each Agent: about (job, personality, voice), the desk (live
  screen, wake, take control, disk), contact (number, mailbox, outgoing
  caps), access (grants for each Connection), memory and work.
- **Conversations** (`/c/<channel>`, `/c/<channel>/t/<message>`) —
  direct and group Channels, with the inspector slot on the right for
  the Desk Panel, a Thread, a Call or a mail message. The inspector of a
  Text Conversation is not built.
- **Runs** (`/runs`, `/runs/<id>`) — a readable timeline for each Run:
  the trigger, the steps as tool cards, and the tokens and the time in a
  footer. Grouped by day, with filter chips that carry counts, and a
  failure reason in plain words.
- **Memory** (`/memory`) — the page list and the learning feed, with a
  diff preview and a revert on each change.
- **Automations** (`/automations`) and **Software** (`/software`) — the
  durable rules and the published packages. Each explains its kind in
  one line and names the Agent that owns it.
- **Settings** (`/settings/<section>`) — the workspace and the system.
  `/settings` opens the first section.

An address belongs to the session that opens it. A sign-out takes the
address back to Home, so the next sign-in opens Home. A page that opens
with no session keeps its address, and the sign-in opens it. A
conversation that is not the person's says that it does not exist: the
daemon answers it as not found, the same as a conversation that never
was.

## Conversation

Consecutive messages from one author group inside five minutes: the
avatar shows once, and the time shows on hover. Tool progress folds into
one collapsible row under the reply, so a timeline is answers and not
terminations. A system event is a hairline strip with an icon, not a
message. Day dividers, an unread marker and a jump-to-latest control
carry a long Channel.

The composer puts attachment chips above the field, takes a drop, offers
slash commands, and shows an explicit voice state: idle, listening,
transcribing. Enter sends, and Shift with Enter starts a line.

An approval is one card: the title, what and where, Approve as the
primary action, an always-allow checkbox where a rule exists, and Deny
as a ghost. A settled card collapses to one line.

## Type, color and shape

`src/tokens.css` holds every color, type size, weight, line height,
tracking, spacing, opacity, radius, shadow, duration and stacking level
in the interface. `src/tokens.test.ts` fails the
build when another stylesheet names one of those values directly.

- **Type.** Inter Variable, loaded with the application and never left
  to a system fallback. Five sizes: `--text-xs` through `--text-xl`,
  with the body at `--text-md`. Four weights (`--weight-regular`,
  `-medium`, `-semibold`, `-bold`), five line heights (`--leading-none`
  through `--leading-relaxed`, body at `--leading-normal`) and three
  trackings (`--tracking-tight`, `-wide`, `-widest`). Mono for
  identifiers only.
- **Spacing.** Padding, margin and gap read a 4px scale with half steps
  under 16px: `--space-0-5` (2px) through `--space-24` (96px). The name
  is the count of 4px units, so `--space-1-5` is 6px. `--space-px` is
  the 1px gap between the bars of a waveform. A negative margin is
  `calc(-1 * var(--space-…))`.
- **Opacity.** `--opacity-disabled` for a disabled control,
  `--opacity-pending` for a message not yet sent, `--opacity-faint` for
  the far point of the call pulse.
- **Color.** A warm neutral ramp of nine steps, one accent for
  selection, focus and a primary action, and four semantic hues:
  working, waiting, on-call and failed. Each hue has a soft fill and an
  ink for text on that fill. Every surface and text token is an alias
  built from the ramp, so the dark theme redefines the primitives alone
  and the rest follows.
- **Borders.** `--border` is the hairline of a divider, a frame and a
  panel edge. It is decoration, so it stays light. The boundary of a
  control (a field, a select trigger, an off switch) is
  `--border-control`, and `--border-control-hover` on hover. Both hold
  3:1 on every ground a control sits on, in both themes. A checkbox and
  a radio stay native: the browser draws their boundary, and their
  checked fill takes `--accent` through `accent-color`.
- **Shape.** Radii at 6, 8 and 12 px, plus a pill and the message
  bubble. Two shadows, rest and raised: a border lifts an input or a
  tile, and a shadow is for a floating layer.
- **Rings.** The focus ring and the presence rings are a gap in the
  ground color and then the hue, so a ring reads on a fill of its own
  hue.
- **Logo.** The mark is three desks that make a P, with the fourth
  place open. `LogoMark` draws it at one em beside the name "Pagis" in
  the sidebar head and on the sign-in card. Its three desks read
  `--logo-top`, `--logo-bowl` and `--logo-bottom`, which the dark theme
  redefines. `assets/brand` holds the same mark as files, with its
  construction and rules.

The Client App's own pages, setup and status, are plain pages in the
app package with no bundler. `desktop/static/pages.css` gives them the
look of the primitives, and it reads only the tokens. The desktop build
copies `tokens.css`, the Inter font and the mark from `assets/brand` into
the package, so the pages follow both themes and load with no network.
`desktop/src/pageDesign.test.ts` fails when that stylesheet names a value
that is not a token.

## Primitives

`src/primitives/` holds the controls every screen renders through:
Button, IconButton, TooltipButton, Input, Textarea, Combobox, Select, Tabs, Menu,
Dialog, Toast, Badge, Avatar, Switch, LogoMark, and the layout frames.
Behaviour comes from Radix Primitives, which gives focus, keyboard and portal
handling; the look comes from the tokens. The Combobox, a field that
also offers a list of choices, takes its keyboard and ARIA behaviour from
Downshift and draws its list in a Radix Popover, because a native
`<datalist>` list grows past the window and does not scroll. A screen
imports Radix and Downshift only through the primitives. Icons are Lucide, inline as SVG, at 14, 16 or
20 px: the glyph of a small, a medium and a large control.

A screen does not write its own control. A control that is missing is
added to the primitives. A screen writes an inline style only for a
value it computes at run time, such as a height or a position; a fixed
look goes in its stylesheet.

## Motion and sound

Motion is 150 ms for a state change, 250 ms for a panel, 400 ms for a
slow move, and a 1200 ms period for an ambient loop such as the working
pulse. The panel easing overshoots, so an inspector arrives with weight
instead of sliding flat. Under a reduced-motion preference every
duration is zero.

There are four sound cues: approval needed, call ringing, call
connected, and run failed. Each one is a pair of short tones the Web
Audio API makes on the spot, so the product ships no audio file and no
decoder. The cues are off by default. The setting lives in the browser or
the Client App that the person listens on, and it alone decides whether
one sounds.

## The standards every screen meets

- Every screen passes WCAG AA contrast in both themes.
- Every control is reachable and operable by keyboard, with a visible
  focus state.
- Every value comes from `tokens.css`. A screen reads a surface or text
  alias, never a ramp step, and a font stack or a shell width only
  through its token.
- The Product App shows in a browser, in the product window of the Client
  App, and in the browser of a phone. Its layout follows ADR-0022 "Layout
  rules" and works at 390 px: the conversation list goes off-canvas at
  760 px and below, where an inspector is a sheet. Above 760 px an
  inspector is a column that never covers the composer, and at 1100 px
  and below the person opens and closes the Desk Panel. A master-detail
  body becomes two steps with a back control. A stylesheet breaks only
  at 760 px and at 460 px, the narrow phone; 1100 px is the one
  breakpoint the code reads.
- Copy follows ASD-STE100 and the user's own vocabulary. It never shows
  an internal name, such as a tool identifier, a protocol or an alias.
- An empty state explains its kind in one line and offers one action.
