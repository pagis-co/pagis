# 0026: The daemon sends anonymous analytics, and only a release build can

Status: accepted.

## Context

Pagis needs to know how many installations run, which releases they run,
what kind of installation they are, and which features they use. It needs
no knowledge of any Person. Self-hosted open-source products that collect
usage data follow a few patterns. Grafana, GitLab Service Ping and Home
Assistant send one aggregate report each day. Homebrew, Next.js and Nuxt
honor an off switch in the environment, and many of them honor
`DO_NOT_TRACK`. The complaints come from event streams about people, from
data sent through third-party scripts in the browser, and from data that
people cannot turn off.

## Decision

### One sender: the daemon

The daemon is the one part of Pagis that sends analytics. The Product App,
the Administration Interface and the Client App send nothing, so a
Person's browser never reaches the analytics service, and the pages load
no third-party script. The daemon sends to PostHog US Cloud through its
capture API, with the HTTP client that the workspace already uses. The
official Rust client adds a second major version of that HTTP client and
features that Pagis does not use.

### Anonymous, and small

- The **Installation ID** is a random value that the daemon makes at its
  first send and keeps in `analytics.json` in the State Directory. It
  comes from no hardware, no host name and no Public Origin. It is the
  `distinct_id` of every event.
- Every event sets `$process_person_profile` to false and
  `$geoip_disable` to true. PostHog makes no person profile and looks up
  no location. The PostHog project discards the client IP address.
- The catalog has three events. `installation_created` goes at the first
  send. `installation_upgraded` goes at the first send of a new release
  and names the release before it. `installation_report` goes once a
  day. Each event carries the release, the operating system and the
  architecture.
- The Installation Report holds the kind of installation, the Storage
  Backend, whether Remote Access is on, whether Docker answers, the number
  of People and of Agents as ranges (0, 1, 2-5, 6-20, 21+), and one flag
  for each feature in use: Connections, Schedules, Event Subscriptions,
  Plugins, Software, Hosts, Agent Phone Numbers and Agent Mailboxes.
- Every property is an enum, a number or a flag. The types allow no
  content, no names, no addresses, no paths and no identifier but the
  Installation ID. A test holds the report to its list of properties.

### Only a release build can send

The release build sets `PAGIS_POSTHOG_PROJECT_ID` and
`PAGIS_POSTHOG_TOKEN` in its environment, and the compiler writes them
into the daemon. The repository holds neither. A build that lacks either
value holds no project, so a build from source and a development build
send nothing, and no switch can turn them on.

### Opt-out, in three places

- **The Analytics System Setting**, on by default. An Administrator turns
  it off in the Administration Interface, and the daemon writes
  `analytics` in `config.toml`. The analytics task reads the setting at
  each check, so the change needs no restart.
- **`DO_NOT_TRACK`**: any value but empty, `0` or `false` stops the
  daemon.
- **The build**: see above.

The Administration Interface says which of these stops the daemon.

### Never in the way

A background task does all of the work. It waits some minutes after the
start, then checks once an hour and sends what is due in one batch. A
request has a short timeout. A batch that does not go costs a debug log
line and goes again at the next check with the same Installation ID.
Nothing that the daemon does waits on the task.

## Consequences

- A new property or event is a change to the catalog in
  `crates/pagis-analytics` and to its test, and it follows the rules
  above.
- The release pipeline must set both variables, or the release sends
  nothing. The Linux `cross` builds pass them through `Cross.toml`, and
  the Headless Server image takes them as build arguments.
- The daily report shows the use of each installation, not of each
  Person. A funnel through the Onboarding, or anything else about one
  Person, is not collected.
- Pagis publishes no dashboard of the data.
