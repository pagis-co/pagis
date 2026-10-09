# 0034: The Product App has a phone layout

Status: accepted.

## Context

A phone is a pager and a remote. Its Person answers an approval, reads a
missed call, checks a failed Run and gives work. A narrow desktop with a
drawer puts those tasks behind navigation controls and an inspector.

Home Assistant Companion and the Slack, Linear and GitHub mobile apps use
a bottom tab bar for their main places, pushed screens for detail and a
bottom sheet for a decision. iOS and Android use large titles, safe-area
insets and touch targets of at least 44 px.

## Decision

### The Product App draws the phone layout at 760 px and below

`useIsMobile()` is the one switch. The Mobile App and a phone browser show
the same Product App. Above 760 px the desktop layout stays. One route
is the exception: the live screen of a Desk also stays in the phone
layout on a touch device that is 760 px tall or less. A person who turns
the phone on its side to see the screen larger stays on the screen. The phone
has no off-canvas sidebar, drawer controls or inspector panel.

### The tab bar holds four places

Home, Conversations, Sprites and You are the tab roots. Home shows the
count of the Needs-You Queue. You holds the Person, server, Memory,
Automations, Software, Settings and the settings of this phone. A Desk
belongs to its sprite. A Run opens from Home, Work or a queue item.

### A screen is a tab root, a pushed screen or a sheet

A tab root has a large title and the tab bar. A pushed screen has a
navigation bar with a back control that names its parent. Back goes to a
fixed parent route, so a Notification opens a screen with a usable way
back. Android's back button closes the top sheet or goes to that parent.

Approval, New group, New sprite and Add a connection are bottom sheets.
Destructive confirmations are action sheets. Radix Dialog and AlertDialog
provide focus trapping, Escape, accessible names and focus restoration.

### Every screen keeps the design system

Colors, spacing, type and surfaces use the Product App tokens. Controls
use its primitives, with touch targets of at least 44 px and 15 px control
text. Both themes meet WCAG AA. Fixed layers respect the safe area.

### The phone shows fewer places

Administration, the sign-in link maker, the Desks list and the Runs place
stay on the desktop. The phone keeps the actions of each place it shows.
Google connects through the Installation OAuth Client and Google's own
account selection; no client ID or secret is entered on the phone.

### The reference design lives in the repository

`docs/design/mobile/` holds one image for each screen at 390 × 844 CSS
pixels, drawn at 2×. A change to a phone screen updates its reference image
in the same pull request.

### Other ways were considered

A separate SwiftUI, Jetpack Compose or React Native interface would give
each client its own product and release cycle. The Product App already
serves every client, so the Mobile App keeps its Capacitor shell.

A fifth tab would make the labels smaller than the design system's
smallest type. Four places keep the labels readable. A Desks tab would
separate a sprite from its Desk, so the profile and conversation open it.

## Consequences

- One server serves the desktop and phone layouts.
- A Person can answer what needs them directly from Home.
- Each place maintains both layouts and their reference designs.
- The Mobile App keeps native notifications, scanning and lock-screen
  answers. The App Store guideline 4.2 risk in ADR-0032 still applies.
