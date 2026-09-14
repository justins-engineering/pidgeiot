# Privacy Policy

Last updated: {{LAST_UPDATED}}

This policy describes what the PidgeIoT platform collects, where it lives, how long we keep it, and what we do (and deliberately don't do) with it.

Questions about anything here: [info@jes.contact](mailto:info@jes.contact).

## Who is responsible for your data

PidgeIoT is operated by Justin's Engineering Services LLC, a Montana limited liability company (Montana Secretary of State registration C1275614), registered as a foreign limited liability company in Massachusetts (Massachusetts identification number 001678120), with its principal office at 30 Virginia Ave, West Springfield, MA 01089. For the account you create, the messages you send us, and the diagnostics your browser sends us, we are the controller of your personal data.

For the data your devices and your team put into the platform, you (or the organization you belong to) are the controller and we process it on your instructions. Where the GDPR, the UK GDPR or the Swiss FADP applies, that processing is governed by our Data Processing Agreement, which is published at [https://pidgeiot.com/dpa/](https://pidgeiot.com/dpa/) and forms part of our Terms of Service. Email [info@jes.contact](mailto:info@jes.contact) if you need a countersigned copy for your own records.

## What we collect

Account data. When you register a dashboard account, our self-hosted Ory Kratos identity system stores your email address and a hash of your password. Your email address is the only thing it requires. It also holds whatever else you choose to give it: a name, a phone number, your choice about product-update email, and any additional sign-in credential you set up (a passkey, an authenticator app or backup codes). We never store your password in plain text.

Device data. The platform exists to hold the data your devices send it: telemetry values, device configuration (shadow state), and device log uploads, along with the metadata you enter when creating flocks and pigeons (names, descriptions, connector settings). You control what your devices report.

Web logs. Like nearly every web service, our infrastructure records standard request logs (IP address, user agent, timestamps, and the routes requested) used for debugging and abuse prevention.

Error diagnostics. If the dashboard hits a bug, your browser sends us a technical report: the error message, the place in our code where it happened and the stack trace that led there, the app build, the page's route template, whether a session was signed in at the time (a yes or no, nothing more), your browser's user agent string, and a short trail of recent in-app actions recorded as request method, route template, and status code. These reports are de-identified by design and contain no direct account identifier. They carry no account identity, no full URLs, no query strings, no form contents, and no request or response bodies, and we do not link them to your session. If you choose to send us a problem report yourself, we attach your account identity to that report so we can follow up with you, and identified reports are deleted with your account. Error reports are kept for 90 days; the long-lived records we keep about error patterns are grouped by error signature and hold a redacted message and code location, not account identifiers.

## What we don't do

- We do not sell your data. Not account data, not telemetry, not anything.
- We do not run third-party advertising or ad-tracking scripts on this site.
- We do not use cookies, browser storage or analytics to profile you or to follow you to other sites. The next section lists everything we do set, and why.

We may derive aggregated statistics about use of the service, such as account, device and message counts, where those statistics do not identify and cannot reasonably be used to identify any customer, device or person. We do not use your data to train models across customers. Any broader use of your data, including cross-customer model training, applies only if you affirmatively enroll in that program under separate terms.

## Cookies, storage and analytics

This is the complete list of what the site stores in your browser or reads back from it. None of it is used for advertising or for cross-site tracking, and nothing on this list is sold or shared with anyone for their own purposes.

- `ory_kratos_session` (cookie): Keeps you signed in. Set by our authentication service on auth.pidgeiot.com and scoped to pidgeiot.com so the dashboard and the API both see it. Marked HttpOnly, so no script can read it. A session lasts 4 hours.
- `csrf_token_<hash>` (cookie): Protects the sign-in, registration, recovery and account settings forms against cross-site request forgery. Set by the same authentication service, one per form, with Domain=pidgeiot.com, HttpOnly and Secure, and a one-year lifetime. That lifetime is the identity software's own default and outlives by far the session it protects.
- `session_expiry` (cookie): Our own hint of when your session ends, so the dashboard can sign you out on time without asking the network. Its value is a timestamp and nothing else: no identifier, no account, no token. It is deliberately readable by this page's script, because the sign-in cookie above is not.
- `theme` (browser storage): Remembers whether you chose the light or the dark theme. Written when you click the toggle.
- `pidgeiot.graphs.v1.*` (browser storage): The telemetry graphs you configure for a pigeon or a flock. They are saved to your account so they follow you between browsers, and kept here as a local copy so they load instantly. Either way they exist only behind the sign-in.
- `pidgeiot.return_to.v1` (browser storage): The page you were on when a sign-in interrupted you, so we can put you back there afterwards. Kept for 30 minutes at most, and deleted the moment it is read.
- `Cloudflare Turnstile` (third-party script, contact page only): Anti-abuse on the public contact form. It loads from challenges.cloudflare.com after the page has rendered, and on no other page of this site.

Analytics. We use Cloudflare Web Analytics to count page views on our public pages. It is not served to visitors connecting from the European Economic Area, the United Kingdom or Switzerland, so if you are in one of those places no analytics script runs in your browser at all. For everyone else it sets no cookies, stores nothing on your device, and does not identify you or follow you to other websites; what it records is the page you viewed, the site that linked you to it, your browser, operating system and device type, the country your connection came from, and how quickly the page loaded.

Separately from that script, and whether or not it runs, our edge provider records the request itself: the standard web log described under "What we collect", kept for the period given in the retention table below.

## Do Not Track and Global Privacy Control

Some browsers send a Global Privacy Control or Do Not Track signal on your behalf. There is nothing here for either signal to switch off: we sell no personal data and share none for cross-context advertising, we serve no advertising and no cross-site tracking, and the one analytics script we run is not served at all to visitors in the European Economic Area, the United Kingdom or Switzerland. If that ever changes, honoring the signal becomes something we have to build rather than something we can simply state, and this section will say so.

## Where your data is processed, and how transfers are protected

We are a United States company, and the platform runs on infrastructure in the United States and on a global edge network. All traffic between your browser or your devices and the platform is encrypted in transit with TLS. In plain terms:

- Each device's own state (its configuration, its latest readings and its log buffer) lives in a Cloudflare Durable Object that is created near whoever first set the device up, and stays there. For a team in Europe that is usually a European data center, but we do not guarantee it.
- Our relational database and our identity database are hosted by Crunchy Bridge on AWS in Northern Virginia (us-east-1).
- Our identity server and our device-transport terminators run on a server in Vint Hill, Virginia.
- Our edge provider runs our code in whichever of its data centers receives a request, and its queues and caches have no fixed location.
- Billing is handled by Stripe in the United States. Stripe collects payment details on its own checkout pages and processes them as an independent controller under its own privacy notice; we do not receive your card number.
- Transactional email is sent through our edge provider's email service, with a second email provider as the fallback.

If you are in the European Economic Area, the United Kingdom or Switzerland, personal data processed through the United States-hosted parts of the platform is currently transferred to the United States. For personal data we process on behalf of our customers and transfer to the United States, we rely on the European Commission's Standard Contractual Clauses (Commission Implementing Decision (EU) 2021/914 of 4 June 2021, Module Two), together with the UK International Data Transfer Addendum for UK data and the Swiss adaptations for Swiss data, as applicable, as the legal basis for those transfers. Those clauses are part of our Data Processing Agreement. We are not certified under the EU-U.S. Data Privacy Framework; some of our service providers are, and we rely on their certification for the part of the processing they do.

Those clauses are a contract between us and a customer, so they cover the personal data our customers transfer to us under the Data Processing Agreement. They do not cover personal data you provide directly to us in our own role as controller, such as your account information, messages you send us and identified diagnostics. Under current European Data Protection Board guidance, that direct disclosure by an individual to a controller outside the EEA is not itself treated as a transfer under Chapter V of the GDPR. We nevertheless apply the technical and organizational measures described in our Data Processing Agreement to protect that data.

We do not offer EU data residency today. If you need it, contact us and tell us the requirement.

Every device has its own key pair, and we verify a device by its stored public key. A device's credentials are held in that device's record and are never shown to you again after they are first issued. A device provisioned for CoAP or MQTT also holds a pre-shared key that authenticates its transport handshake, which the terminator for that transport reads; it can be rotated from the dashboard.

## Service providers we use

We use a small number of service providers to run the platform. The current list, with what each one does, where it processes data, and the transfer safeguard that covers it, is published at [https://pidgeiot.com/subprocessors/](https://pidgeiot.com/subprocessors/) and forms part of our Data Processing Agreement. We give customers thirty days' notice by email before we add or replace one, except an urgent replacement needed to keep the service secure or available, which we notify as soon as we can.

## How long we keep data

We keep data for as long as it serves the purpose it was collected for, and no longer. The concrete periods are:

| Data | How long, and what happens then |
|---|---|
| Your account (email, name, phone if you give one, credentials) | While your account exists. Deleted when you ask us to delete it. |
| Sign-in sessions | 4 hours, then they expire. |
| Verification and recovery codes | Minutes to hours, and single use. The record that the message was sent stays in the identity system's own log. |
| Organization invitations | 7 days, and single use, then they expire. |
| Device configuration, latest readings, device log buffer | While the device exists, and the log buffer keeps only the newest 200 chunks. Erased when you delete the device. |
| Telemetry history | 7 days on the free tier, 30 days on Builder, 90 days on Growth, 13 months on Scale and Fleet, by the plan the organization is served at (the free tier while a paid plan is suspended). Deleted automatically after that, or when you delete the device, whichever comes first. |
| Saved dashboard graphs | While your account exists. Deleted when you delete the graph, and with the rest of your account when you ask us to delete it. |
| Firmware images | Until you ask us to remove them, which can be after the fleet they belonged to is gone; there is no delete button yet. Firmware storage is intended for your device firmware and related binaries rather than account or telemetry data. |
| Billing records (invoices, subscription history) | As long as tax and accounting law require, held by our payment processor. Deleted at the end of the statutory period. |
| Contact-form and support messages | Kept as correspondence you addressed to us. Deleting your account detaches your account identifier from the message rather than deleting the message itself. |
| Dashboard error reports | 90 days, then deleted automatically. The long-lived records we keep about error patterns are grouped by error signature and hold a redacted message and code location, not account identifiers. |
| Web and API request logs | 7 days, then deleted automatically by our edge provider. |
| Logs on our own server (identity service, device transport) | Kept in the server's system journal and deleted after 30 days. |
| Your product-update choice | The record of when you gave or withdrew consent is kept while your account exists, so we can show we had it, and deleted with your account. |
| Backups of our databases | Rotated on our database host's own schedule. Deleted data disappears from a backup when that backup expires. |

## Why we are allowed to process your data

If you are in the EEA, the UK or Switzerland, the law requires us to tell you the legal basis for each kind of processing:

- **To create and administer your account**, authenticate you, give you access to the service, whether you use it in your own name or on behalf of your organization, and provide the features you request: our legitimate interest in providing and administering the service (GDPR Article 6(1)(f)). Where you use the service on behalf of an organization and we process device or other personal data on that organization's behalf, we do so as a processor under the Data Processing Agreement and your organization is responsible for the applicable legal basis.
- **To create and run a paid subscription**, calculate and process subscription and usage charges, and administer billing: where you are the customer contracting with us, performance of our contract with you (GDPR Article 6(1)(b)); where you act on behalf of a separate organization that is the customer, our legitimate interest in administering that organization's subscription and billing (Article 6(1)(f)).
- **To keep tax and accounting records**, including validating an EU VAT number you give us against the European Commission's VIES register: a legal obligation (Article 6(1)(c)).
- **To keep the platform secure and working** (request logs, rate limiting, de-identified error diagnostics, notifying ourselves of failures): our legitimate interest in running a secure service (Article 6(1)(f)). We have designed these to carry as little personal data as possible; error reports carry no identity unless you choose to attach one.
- **To answer your messages** when you use the contact form or send feedback: our legitimate interest in responding to you, and, where you are asking about becoming a customer, steps you ask us to take before a contract (Article 6(1)(b) and (f)).
- **Email updates**: only with your consent, which you can withdraw at any time (Article 6(1)(a)). We do not send marketing email today.

## Product updates by email

If you tick the box for product updates, we send you occasional email about PidgeIoT. We do that only because you asked us to, which in legal terms means we rely on your consent (GDPR Article 6(1)(a)), and you can withdraw it at any time in your account settings without giving a reason and without affecting anything else about your account. Withdrawing takes effect for anything we have not already sent. We do not send this email unless you have asked for it, we do not share your address with anyone else for their own marketing, and every message we send includes a link to stop them.

## Objecting to how we use your data

You can object at any time to our sending you marketing email, and we will stop; this is an absolute right and we do not weigh it against anything (GDPR Article 21(2) and 21(3)). You can also object, based on your particular situation, to processing we carry out because of our legitimate interests, including account administration, security and diagnosing faults. If you do, we will stop that processing unless we have compelling legitimate grounds that override your interests, rights and freedoms, or the processing is needed to establish, exercise or defend legal claims. If an objection concerns processing needed to operate your account or a feature you use, we may not be able to continue providing that part of the service. Email us at the address in this notice, or use your account settings for the marketing choice. Objecting costs you nothing.

## Your rights

If you are in the EEA, the UK or Switzerland, you have the right to ask us for access to the personal data we hold about you, to have it corrected or deleted, to restrict or object to how we process it, where the law gives you that right, to receive it in a portable format, and, where we rely on consent, to withdraw that consent. You also have the right to complain to your data protection authority.

Much of this you can do yourself:

- **See and correct** your email, name and phone number in account settings.
- **Delete** devices, empty fleets and empty organizations in the dashboard, and your own identified error reports through the API.
- **Take your data with you**: every fleet, device, configuration and telemetry history you can see in the dashboard is available as JSON through the API documented on our [API reference](https://pidgeiot.com/api-reference/) page, and you can configure a forwarding endpoint to receive your telemetry continuously.

For anything else, email [info@jes.contact](mailto:info@jes.contact) from the address on your account. We will confirm receipt within five business days and answer within one month; if a request is complex we may take up to two further months and will tell you why. Deleting your account is the exception: it is completed within 30 days, as described under "Deleting your data". We do not charge for this unless a request is clearly unfounded or excessive.

If your data reached us as part of a customer's use of the platform, for example through a device your employer operates or other data your organization puts into the service, that customer is the controller and we will pass your request to them.

## Deleting your data

You can delete your pigeons in the dashboard at any time, a flock once it holds no pigeons, and an organization once it holds no flocks; deleting a pigeon removes its stored shadow, telemetry, and logs from the platform.

There is no automated account-deletion flow yet. To delete your account, email [info@jes.contact](mailto:info@jes.contact) from your account's address and we will remove it within 30 days, together with the data the retention table says is deleted with your account. Data that the retention table says is retained separately, including records we must keep for legal, tax or accounting purposes, is handled as that table describes.

## Automated decisions

We do not make decisions about you by automated means that have legal or similarly significant effects. Two automated checks exist and you should know about them: when an organization saves an EU VAT number we validate it against the European Commission's VIES register and will not accept a number the register says is invalid; and when a free-tier account exceeds its monthly message allowance, its devices' uploads are paused until the next period. Neither is a decision about you as a person: the VAT check concerns an organization's VAT number, and the free-tier pause applies to an account's device uploads based on its usage allowance. Either can be raised with us by email.

## Telemetry forwarding you configure

PidgeIoT lets you configure a forwarding endpoint for a pigeon's telemetry. If you do, we send that pigeon's telemetry to the endpoint you configured instead of storing its history with us. That endpoint is chosen and controlled by you: data sent there is governed by whoever operates it, not by this policy.

## Email

We send transactional email only: verification and recovery codes, organization invitations, the alert notifications you configure, a warning when a free-tier account reaches 80% of its monthly message allowance, and, on paid plans, the receipts and invoices our payment processor sends. Delivery goes through our edge provider's email service, with a second email provider as the fallback; whichever delivers a message necessarily processes the recipient address and its content in order to do so.

We do not send marketing email today.

## Changes to this policy

As the platform evolves we may update this policy. Changes will be posted on this page with a revised "Last updated" date, and material changes are notified by email to the address on your account at least 30 days before they take effect.
