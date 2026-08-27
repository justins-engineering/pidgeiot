# Security Policy

PidgeIoT is built and operated by Justin's Engineering Services LLC. This
policy covers `pidgeiot` and the rest of the platform listed below, and it is
the same document in every PidgeIoT repository.

## Scope

Source repositories:

- [pidgeiot](https://github.com/justins-engineering/pidgeiot), the edge backend, dashboard and shared models
- [pigeon](https://github.com/justins-engineering/pigeon), the Zephyr device client library
- [pigeon-examples](https://github.com/justins-engineering/pigeon-examples), sample applications for that library
- [pigeonhole](https://github.com/justins-engineering/pigeonhole), the MQTT broker
- [loft](https://github.com/justins-engineering/loft), the CoAP terminator
- [embedded-departure-board](https://github.com/justins-engineering/embedded-departure-board), the transit departure board firmware

Hosted services:

- `https://pidgeiot.com`, the dashboard and public site
- `https://api.pidgeiot.com`, the platform API and device HTTP endpoints
- `https://auth.pidgeiot.com`, the identity provider
- `coap.pidgeiot.com`, the CoAP device endpoint on UDP and TCP port 5684
- `mqtt.pidgeiot.com`, the MQTT device endpoint on TCP port 8883
- `https://status.pidgeiot.com`, the status page

Anything not on those two lists is out of scope, including the third party
providers the platform runs on. Report an issue in one of those to its own
vendor.

## Reporting a vulnerability

Email **security@pidgeiot.com**.

Please do not open a public issue, pull request or discussion for a suspected
vulnerability, and please do not publish details before the disclosure window
below has run.

Include as much of this as you have:

- What the issue is, and why you believe it is a security problem rather than a bug.
- Which repository, host or endpoint is affected, plus the version or commit if you know it.
- Steps to reproduce, ideally as a minimal request sequence or a short script.
- What an attacker gains: the data, account or device they reach, and from what starting position.
- Supporting output, logs or screenshots. Redact credentials, tokens and personal data before sending them.
- How you would like to be credited, or that you would rather not be.

One report per issue, please.

## What to expect

- We acknowledge a report within **3 business days**.
- We send an initial assessment, including whether we consider the report in scope and how we rate its severity, within 10 business days.
- We send a progress update at least every 14 days while a report is open.
- We ask you to hold public details for **90 days** from the day we acknowledge the report, or until a fix ships, whichever comes first. If a fix will take longer than that we will tell you and propose a date. If we find an issue is being exploited we will move faster and say so.
- We credit you in the release notes or advisory for the fix unless you ask us not to.

There is **no bug bounty** at this time and we cannot offer payment for
reports.

## Safe harbor

We consider security research and vulnerability disclosure that follows this
policy to be authorized. We will not pursue or support legal action against
anyone who reports an issue to us in good faith and stays inside the scope and
limits described here, and if a third party brings a claim against you for
work that stayed inside this policy, we will make it known that the work was
authorized.

Good faith comes first. If you are not sure whether something is in scope, ask
before you test.

## What not to do

- Do not test against data, accounts, organizations, flocks or devices that are not yours. Sign up for your own account and register your own devices.
- Do not run denial of service, stress, load or volumetric tests against any hosted endpoint, including the device transports and firmware downloads. Describe a rate limiting or resource exhaustion concern in a report rather than demonstrating it at scale.
- Do not social engineer our staff, customers or vendors. That includes phishing, pretexting and any attempt at physical access.
- Do not access, copy, alter or destroy data that is not yours. Stop as soon as you have enough to prove the issue, and tell us what you touched.
- Do not point automated scanners at the hosted services without asking first. Scanning the source is fine; scanning production is not.
- Do not publish before the disclosure window closes.

These are usually accepted but ranked low on their own, without a demonstrated
impact: missing security headers, missing SPF, DKIM or DMARC records, output
from an automated scanner with no verified finding, version disclosure, and
reports that depend on an already compromised device or browser.

## Supported versions

Security fixes land on the `main` branch of the affected repository first and
ship in that repository's next published release. The hosted service runs
from `main`.

| Version | Supported |
| --- | --- |
| `main` | Yes |
| Latest published release | Yes |
| Any earlier release | No |
| Forks and vendored copies | No |

There is **no long term support branch and no backporting**. If you run a
published release or a fork, the upgrade path for a security fix is to move to
the current release or to `main`. Devices in the field take firmware fixes
through the platform's own firmware update mechanism.

## security.txt

Each hosted origin serves an [RFC 9116](https://www.rfc-editor.org/rfc/rfc9116)
`security.txt` pointing back at this policy:

- `https://pidgeiot.com/.well-known/security.txt`
- `https://api.pidgeiot.com/.well-known/security.txt`
- `https://status.pidgeiot.com/.well-known/security.txt`

All three are served from one source in the `pidgeiot` repository. The text
lives at `fancier/public/.well-known/security.txt.unsigned`, and the document
the origins answer with is `fancier/public/.well-known/security.txt`: either a
copy of that source or the same text under an OpenPGP cleartext signature. A
unit test compares the two payloads and the release build runs it, so a served
copy that an edit left behind fails the build instead of shipping.

## OpenPGP key

The document's `Encryption` field names our OpenPGP key, published at
`https://pidgeiot.com/.well-known/pgp-key.txt`. Its fingerprint is:

```
2ADE 9368 178A 62EE 99B3  5615 DDE1 CA3C E883 F7B2
```

Compare that against this policy on GitHub before you trust a copy you
downloaded from the site, since the site is the thing you might be reporting
a problem with. The key's user id reads `ops@jes.contact`, which is where
`security@pidgeiot.com` delivers; it is the right key even though the two
addresses are spelled differently.

Use it to encrypt a report, and, when the served document carries a signature,
to check that signature:

```sh
curl -s https://pidgeiot.com/.well-known/pgp-key.txt | gpg --import
curl -s https://pidgeiot.com/.well-known/security.txt | gpg --verify
```

## Renewing Expires

The document carries an `Expires` date, currently `2027-02-26`, and has to be
renewed before that date passes, otherwise the document is stale by its own
terms. Change the date in the unsigned source, then bring the served copy back
into step with it. If the served document is signed, that means signing it
again: the signature covers the date being renewed, so editing the served file
in place would leave a signature that no longer verifies. From the repository
root:

```sh
gpg --local-user 2ADE9368178A62EE99B35615DDE1CA3CE883F7B2 --clearsign --yes \
  --output fancier/public/.well-known/security.txt \
  fancier/public/.well-known/security.txt.unsigned
gpg --verify fancier/public/.well-known/security.txt
```

`--yes` is there because the previous document is still in place and `gpg`
would otherwise stop to ask before replacing it. If the served document is not
signed, copy the source over it instead. Either way the unit test above fails
the build until the two match, so nothing ships half renewed.
