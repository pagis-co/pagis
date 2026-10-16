# Architecture decision records

Each record states one standing decision and names the parts that are not
built. Read the records that touch an area before you change it.

## The agent and the Run

- [0001](0001-own-the-agent-loop.md): Pagis owns its agent loop
- [0002](0002-a-run-is-the-work-of-a-resident-actor.md): A Run is the work of a resident actor
- [0003](0003-threads-are-the-agent-to-agent-protocol.md): Threads are the agent-to-agent protocol
- [0004](0004-a-block-that-asks-is-a-request.md): A block that asks is a Request, and a block that shows is content
- [0005](0005-stable-capability-snapshots-behind-one-broker.md): One broker serves tools from a stable Capability Snapshot
- [0006](0006-a-wake-up-stands-between-a-trigger-and-a-run.md): A durable Wake-up stands between a trigger and a proactive Run
- [0036](0036-the-daemon-resumes-unfinished-runs-from-their-journal.md): The daemon resumes unfinished Runs from their journal

## Memory

- [0007](0007-memory-is-files-under-git.md): Memory is Markdown files under git
- [0008](0008-memory-reads-through-derived-indexes.md): Memory reads through derived indexes, and a Forget purges every copy
- [0009](0009-conversation-context-and-learning-have-separate-checkpoints.md): Conversation context and learning have separate checkpoints
- [0010](0010-reflection-reconciles-selected-evidence.md): Reflection reconciles selected evidence
- [0011](0011-an-arrival-brings-a-source-into-memory.md): An arrival brings a source into memory, and a Reflection Filter decides what reflects

## Accounts, secrets, Computers and tools

- [0012](0012-the-installation-holds-the-oauth-client.md): The installation holds the OAuth client, and the daemon serves the catalog
- [0013](0013-the-daemon-types-secrets-the-model-never-sees.md): The daemon keeps the secrets and types them where the model never sees them
- [0014](0014-the-computer-is-pixels-and-pagis-owns-the-stream.md): The Computer is pixels, and Pagis owns the stream
- [0015](0015-a-host-action-runs-on-a-present-client.md): A host action runs on a present client, never in the daemon
- [0016](0016-a-software-package-is-a-tag-addressed-version.md): A Software Package is a tag-addressed Version
- [0017](0017-a-plugin-consumes-connections.md): A Plugin consumes Connections and never exposes one
- [0033](0033-a-coding-session-is-a-guest-coding-harness-that-an-agent-supervises-over-acp.md): A Coding Session is a guest Coding Harness that an Agent supervises over ACP

## Agent identity and channels

- [0018](0018-agents-register-with-their-own-identity.md): An Agent registers with its own durable identity
- [0019](0019-an-agent-mailbox-is-the-agents-own-identity.md): An Agent Mailbox is the Agent's own identity
- [0020](0020-pagis-is-a-sip-endpoint-and-a-texting-client.md): Pagis is a SIP endpoint and a texting client of one Org carrier
- [0021](0021-caller-id-proposes-a-tier-and-a-keypad-code-confirms-it.md): Caller ID proposes a tier, and a keypad code confirms it

## The desk and the installation

- [0022](0022-the-desk-puts-one-conversation-first.md): The desk puts one conversation first
- [0023](0023-one-org-holds-the-people-of-an-installation.md): One Org holds the People of an installation
- [0024](0024-a-server-serves-a-network.md): A server serves a network behind a proxy, with a separate Administration Port
- [0025](0025-the-client-app-installs-one-exact-server-or-connects-to-one.md): The Client App installs one exact server or connects to one
- [0026](0026-the-daemon-sends-anonymous-analytics-from-a-release-build.md): The daemon sends anonymous analytics, and only a release build can
- [0027](0027-the-client-app-installs-updates-and-upgrades-its-installation.md): The Client App installs Updates, and its Local Installation upgrades with it
- [0028](0028-remote-access-runs-through-the-owners-tailscale.md): Remote Access runs through the owner's Tailscale, and a client signs in with a Sign-In Link
- [0029](0029-a-computer-can-exit-through-its-persons-host.md): A Computer on a server can exit to the internet through its Person's Host
- [0030](0030-a-notification-is-a-web-push-to-each-push-subscription.md): A Notification is a Web Push to each Push Subscription of the Person
- [0031](0031-an-administrator-can-capture-the-model-requests-of-a-run.md): An Administrator can capture the model requests of a Run
- [0032](0032-the-mobile-app-is-a-native-shell-around-the-product-app.md): The Mobile App is a native shell around the server's Product App
- [0034](0034-the-product-app-has-a-phone-layout.md): The Product App has a phone layout
- [0035](0035-the-javascript-projects-are-one-pnpm-workspace.md): The JavaScript projects are one pnpm workspace
- [0037](0037-each-release-ships-the-notices-of-its-third-party-software.md): Each release ships the notices of its third-party software
- [0038](0038-the-routers-build-without-the-native-parts.md): The routers build without the Native Parts

## Workspaces and shared communication

- [0039](0039-each-record-has-one-owner-that-decides-access.md): Each record has one owner that decides access
- [0040](0040-the-workspace-mailbox-starts-with-agent-triage.md): The Workspace Mailbox starts with Agent triage
- [0041](0041-a-workspace-has-one-number-that-its-chief-of-staff-answers.md): A Workspace has one number that its Chief of Staff answers
- [0042](0042-each-workspace-holds-a-calendar.md): Each Workspace holds a calendar
- [0043](0043-messages-between-workspaces-follow-disclosure-rules.md): Messages between Workspaces follow Disclosure Rules

- [0044](0044-a-legal-hold-stops-each-delete-in-a-workspace.md): A Legal Hold stops each delete in a Workspace
- [0045](0045-search-sends-the-query-to-each-workspace.md): Search sends the query to each Workspace
- [0046](0046-a-model-gives-a-second-opinion-on-uncertain-mail.md): A model gives a second opinion on uncertain mail
