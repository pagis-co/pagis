# 0046: A model gives a second opinion on uncertain mail

Status: accepted.

## Context

The mail host supplies the Spam Verdict before the Trust Tier (ADR-0040).
Rspamd uses greylist and reject thresholds to separate uncertain scores
from clear results. A model can add evidence in that band. Message text
can also contain instructions that try to control the model.

The installation uses Model Aliases for model selection and Usage Records
for the Workspace Spend Cap (ADR-0023).

## Decision

The host score has three bands: Clean below 4.0 points, Uncertain from 4.0
inclusive to 15.0 exclusive, and Junk at 15.0 or more. The host's Junk
folder and Reject action always mean Junk. Scores use thousandths of a
point.

Only Uncertain mail asks for a Mail Opinion. Its fixed schema contains a
label (`spam`, `not_spam`, or `unsure`), integer confidence from 0 through
100, and at most three reasons of at most 160 characters each. Unknown
fields, surrounding text and values outside these bounds give no opinion.

The final score adds confidence times 3.0 points for `spam`, subtracts it
for `not_spam`, and adds zero for `unsure`. The host action stays. The
Spam Verdict keeps the opinion and a reason line. Score-only mode moves
nothing. The model never supplies the verdict alone.

The system prompt is a constant. Sender, subject and a text excerpt enter
one JSON object in the user turn. The excerpt has at most 4000 characters;
the sender and subject each have at most 1000. The request offers no tools
and asks for the fixed JSON schema. Strict parsing and bounded weight
limit an answer that follows message instructions.

The `mail-opinion` Model Alias uses the installation's model setup and
appears in Settings under Models. Its preference starts with the same
small text model as the default route. An absent or empty route gives no
opinion. A refused or failed call leaves the host verdict in place.

Before a call, `cap_stop` checks the Workspace with the route candidates.
A stop gives no opinion and writes its reason to the log. A completed call
writes one Usage Record with no Run, including when the answer is invalid.
Workspace totals include these records. Per-Run lists exclude them.

A model request names its caller as a Person or a Workspace. Runs keep
their Person identity. Automatic mail calls name the Workspace, including
Shared Workspaces with no owner Person. The installation key makes an
opaque identity with a separate Workspace prefix. Message fields cannot
change this identity.

Each read computes the host verdict again. Only an Uncertain host verdict
uses the stored Mail Opinion. When no opinion is stored, Uncertain mail
asks the model. Clean and Junk host verdicts use no opinion. The Trust Tier
uses the current reader and Trust List. Message-ID keeps the assessment
key across folder moves.

## Other ways were considered

- A model-only verdict gives message text too much control.
- A model call for every message uses more spend without the host band.
- A separate model client duplicates routing, prices and credentials.
- A synthetic Run gives the Person a Run that no Agent performs.

## Consequences

- The host score remains the main input, and a Mail Opinion is visible in
  the message details.
- The Workspace pays for model opinions through the same Spend Cap reads.
- An unavailable model does not prevent mail intake.
- Usage Records can belong to a Workspace without a Run.
