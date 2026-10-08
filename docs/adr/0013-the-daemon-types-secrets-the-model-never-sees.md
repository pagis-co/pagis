# 0013: The daemon keeps the secrets and types them where the model never sees them

Status: accepted.

## Context

An Agent must sign in to external sites on its own Computer, and a secret must
never reach the model: not in a prompt, a tool result, a kept screenshot or a
container file. The browser's own password manager cannot be the store: the
Computer image has no keyring, so the browser falls back to a store whose key
is public, on the Agent's own volume. The browser profile keeps sessions; the
daemon keeps secrets.

1Password fills from an item that carries its own website and opens that
address before it fills. Apple Passwords matches on the registrable domain,
keeps cross-brand exceptions in a list, and publishes password rules because
sites refuse generated passwords. Playwright supplies credentials from the
runner. None of them matches on a URL path.

The daemon also needs a place for every other secret of the installation: model
keys, carrier keys, refresh tokens.

## Decision

### One encrypted file holds the secrets

Every platform keeps its secrets in one file, `secrets.enc`, in the data
directory, sealed with XChaCha20-Poly1305 under the Installation Key and
rewritten whole on each change. Only the place of the key differs.

On macOS the key is one generic password in the user's default keychain,
service `pagis`, account `safe-storage`, as Chrome, Electron and Signal keep
theirs. A server on macOS reads it at boot and makes it at the first boot. The
keychain grants access per item and per code signature, so one item asks once
for each binary, where one item for each secret would ask once for each secret.
The trust boundary is the same: the file is readable by the owner alone.

A server on Linux reads the key from a Key File. `secrets.key_file` in
`config.toml` names the path, default `/run/secrets/pagis-secrets-key`. The
daemon reads it once at start, refuses a file that a group or other user can
read, and refuses to start without one, because a second key would orphan the
first. The key never sits in the config file, which is the file an operator
copies and shares. Docker, Kubernetes and systemd all deliver a secret file.
A server has no desktop keyring, and an environment variable leaks into `ps`,
crash reports and child processes.

A local installation (`--local`) keeps the key in the platform keyring: the
keychain item on macOS, or the same item through the Secret Service on Linux
(GNOME Keyring, KWallet, KeePassXC), through the `keyring` crate. On Linux the
crate links libdbus, so the machine needs a session bus and a keyring daemon.
The order keeps one key for the life of the data:

1. A generated Key File, `installation-key`, mode 600, in the state directory
   wins, so a keyring that starts to answer later never replaces the key.
2. The keychain or Secret Service item, which the first start makes when the
   keyring answers.
3. A new Key File, when it does not answer.

The Secret Service does not answer with no session bus, no keyring daemon or a
locked keyring. The keychain does not answer when the crate has no access to it
or cannot show its prompt (`errSecInteractionNotAllowed`, as over SSH). The
start log says which place holds the key. A keyring that answers and fails
stops the start, and so does a denied or canceled macOS prompt: the daemon
makes no Key File then. A `secrets.enc` whose key is in neither place stops the
start with a message that names the file. A Key File already in the state
directory keeps a development build, whose ad-hoc signature the keychain does
not know, from any prompt. The generated Key File beside the file it seals is
weaker than a keyring; it is readable by the owner alone, and a Backup never
carries it.

### One data key for each Workspace

The store holds one Tenant Data Key for each Workspace, as an entry of
`secrets.enc` under a name that carries the Workspace. It seals Credential
secrets, one-time code seeds and the refresh token of a Google Connection.
Unsealing a row unwraps the key of the owning Workspace alone, so a row read
across the tenant line stays ciphertext.

A secret name carries the Workspace, because an alias or a mailbox address is
unique only inside a Workspace. An installation secret carries none: a model
provider key, the key and SIP password of an Installation Connection, and the
Installation OAuth Client secret. The Org shares each, so a rotation is one
write.

### The vault

The Workspace vault holds Credential records: the Workspace, the domain, the
username, the login address, the secret, an optional TOTP seed, the applied
password recipe, an owner and a provenance. The login address must be `https`,
and its registrable domain, from the public suffix list, must equal the domain.
The daemon checks both at write and before each fill.

The vault has no export and no reveal, because an export widens the loss that
unique secrets bound. A lost store costs one password reset for each domain
through the Agent's mailbox.

### The daemon owns the navigation of a fill

A fill never fills the page the browser shows. The daemon takes the input
switch, opens the login address in its own tab and waits for the load event.
The Agent chooses which Credential; the record chooses the page. Before it
writes, the daemon checks that the top-level address after redirects is
`https`, that its registrable domain equals the Credential's, and that the
field is an editable field in the top frame: text or email for the username,
password for the secret. A failed check writes nothing and reports why. A
single sign-on host on another domain needs its own Credential. There is no URL
pattern language: two entry points need two records.

### The daemon writes through a browser channel it owns

The daemon never sends a secret as keystrokes to the focused window. The
channel is the Chrome DevTools Protocol over a pipe (`--remote-debugging-pipe`).
screend starts Chromium and holds the pipe; the daemon drives it through the
authenticated control port, and screend answers only while the daemon holds
the switch. The browser opens no debugging port. screend writes only while the
top-level origin is the one the daemon verified.

When the page focuses a field, the username goes into it (a text or email field
in the top frame) and the secret into the password field after it in the same
form. A focused field of another kind or outside the login form stops the fill.
While no input has focus, screend asks again for a few seconds, then finds the
fields itself: the first password field on screen and the last visible text or
email field before it in the same form. screend finds every field before it
writes. It writes each value in one evaluation in an isolated world of the top
frame: focus the field, check the origin and the focus, write, with no page
script in between. A page that moves focus away from its password field does
not get the secret.

### The input switch has a third holder

The input holder is the viewer, the Agent or the daemon. While the daemon holds
it, the screen lease denies the Agent, screend gates its input route and browser
channel on the switch and refuses the daemon's own input batches, and capture
that can reach the model (the stored frame and Run screenshots) is suppressed.
The live view stays, because the user is trusted. Handback takes a new
screenshot. The grant check and the approval resolve before the switch flips,
so the daemon never holds the switch across a parked Run.

The browser and screend run under a uid that the Agent's terminal is not, so
the compositor socket and the pipe, which has no name and no port, are out of
the Agent's reach.

### Managed policies keep the filled secret off the screen

After a fill the Agent drives the same browser. Managed policies
(`computer/chromium-policies/pagis.json`) put `devtools://*`, `javascript:*`,
`view-source:*` and `chrome://inspect/*` in `URLBlocklist`, so F12 opens no
panel and a typed `javascript:` address or bookmarklet does not run. These are
separate Chromium checks from the one that gates the DevTools pipe, so the fill
still works. `DeveloperToolsAvailability: 2` also gates the pipe, so it is not
used. A page's own `javascript:` links and form actions still run, and a page
whose address merely contains "view-source" loads. A site's own show-password
control can still put the secret on screen; no policy closes it.

### Tools, secrets and grants

`pagis-vault` owns the records, the minting, the encryption and the fill:

```text
vault__list(domain?)                                  -> [{id, domain, username, kind}]
vault__create(domain, username, login_url, rules?)    -> {id}
vault__fill(id)                                       -> filled | failed
vault__totp_fill(id)                                  -> filled | failed
vault__delete(id)                                     -> ok
```

No tool returns a secret. The handle is the durable Credential id, because a
confirm field and a later Run need the same secret; the id is not a secret, and
the Grant is the gate. A fill reports success or the check that failed. The
Agent submits the form and reads the screen.

A create mints a unique random secret, so a leak reaches one account. The
optional rules use Apple's password-rules grammar and describe what the site
forbids. The daemon mints the strongest secret the rules allow: the full
allowed set at the maximum length. The default is 32 alphanumeric characters.
The record stores the applied recipe.

A TOTP fill does not navigate. It checks the current top-level address with the
fill rules and writes the current code into the focused text, email or
one-time-code field of the top frame; with no focus, into the one one-time-code
field, else the one text field on screen; with several, nowhere. The Agent
never sees the seed or the code. The user gives the seed when adding the
Credential.

A credential Grant's scope is a list of registrable domains; an empty list
means ask each time. The first use on an ungranted domain raises a
`credential_action` card with the action, domain, username and login address,
and **Approve once**, **Always allow the domain** and **Deny**. Rules are capped
at five, there are no deny rules, and revoking the Grant turns it off. The
daemon derives a rule from the record, not from the Agent's text. A denial
writes nothing.

Every Credential carries an owner and a provenance of `user_supplied` or
`agent_minted`, with its creation time and Run. A Credential is a Workspace
resource: archiving an Agent leaves its minted Credentials under the user. Each
fill and create appends one audit fact: the Agent, the Credential, the domain,
the login address, the Run and turn, the Grant revision, the approval id, and
`filled` or `failed` with the reason, never the secret or a frame.

The user adds an existing login in the Vault settings, and the daemon fills
it. A takeover sign-in is the fallback for a site the fill cannot complete.
Cookie import is refused: it hands live sessions to an Agent, breaks on browser
storage changes, and gives no decision for each site.

## Consequences

- A secret reaches the browser only through the pipe that screend and Chromium
  hold, and a focused terminal or other window gets nothing.
- The Agent cannot aim a fill at a page of its choice. A form reached in the
  middle of a flow cannot be filled; the Agent starts again from the record's
  address.
- A Credential needs the public suffix list.
- A Linux server needs a Key File before the daemon starts. A local
  installation needs no step, and where no keyring answers its key sits beside
  the file it seals. A denied macOS keychain prompt stops a local start.
- A site that signs in over `http` gets no Credential.

## Out of scope

Account creation, TOTP enrolment at signup, secret rotation, shared-credential
equivalence lists and cookie import.
