# PidgeIoT sub-processors

Last updated: {{LAST_UPDATED}}

This is the list Annex III of our [Data Processing Agreement](https://pidgeiot.com/dpa/) points to, and it forms part of that agreement. Changes are announced under Section 6 of the DPA.

Justin's Engineering Services LLC ("the Provider") uses the following third parties to process personal data on behalf of its customers. Each row names the legal entity, what it does for the Service, what data reaches it, where that processing happens, and the legal mechanism that covers the transfer out of the EEA, the UK and Switzerland.

Each sub-processor below processes for the term of the Agreement and until the data is deleted or returned under Section 8 of the DPA.

## The list

### 1. Cloudflare, Inc.

| | |
|---|---|
| **Status** | Active sub-processor |
| **Legal entity** | Cloudflare, Inc. (United States) |
| **Service** | Edge application platform: Workers (the API router), Durable Objects (per-device authoritative storage), R2 (firmware images), Queues (telemetry in flight), Workers KV (status page state), Hyperdrive (database connection pooling and a short-lived query cache), Email Service (the transport for platform and identity mail), Cloudflare Access and Tunnel (administrative gating, identity-server ingress), rate limiting, DNS, CDN, TLS termination, Workers Logs |
| **Data processed** | All request traffic to the API and dashboard; per-device shadow, latest telemetry, device logs, access lists and device public keys; telemetry history in flight; firmware binaries; recipient address and message content of outbound mail; request metadata in invocation logs (retained 7 days) |
| **Processing location** | Cloudflare's global network. Worker code runs in whichever data center receives a request. Each Durable Object is created near the first request that touches it and does not move; no jurisdiction restriction is set today. R2 has no jurisdiction set. Queues, KV and the Hyperdrive cache have no residency control on the Provider's plan. United States and European Union locations are both in normal use. |
| **Transfer mechanism** | Cloudflare's Data Processing Addendum, Version 6.4 effective April 3, 2026, which incorporates the EU SCCs (Module Two where the customer is a controller, Module Three where a processor), the UK Addendum, and states that Cloudflare complies with the EU-U.S. Data Privacy Framework. Cloudflare's Trust Hub states the DPA "is incorporated by reference into our Self-Serve Subscription Agreement", which is the agreement the Provider's plan is on. |
| **DPF** | **VERIFIED Active Participant** (Cloudflare, Inc.): EU-U.S. DPF Active (orig. 2016-11-09), UK Extension Active (orig. 2023-09-28), Swiss-U.S. Active (orig. 2017-11-16); all next due 2026-09-23. HR and Non-HR data; verification method Self-Assessment; Non-HR recourse TRUSTe. List entry verified against the official list on 2026-09-01. Participant page: https://www.dataprivacyframework.gov/participant/5666. Re-check after 2026-09-23 (recertification due). |
| **Vendor sub-processors** | https://www.cloudflare.com/gdpr/subprocessors/ (fetched 2026-08-25; hub page linking the per-service lists) |
| **Certifications** | ISO 27001, ISO 27701, ISO 27018, SOC 2 Type II, PCI DSS Level 1, EU Cloud Code of Conduct, C5 (Trust Hub, fetched 2026-08-25) |
| **Citations** | DPA: https://www.cloudflare.com/cloudflare-customer-dpa/ (fetched 2026-08-25). GDPR Trust Hub: https://www.cloudflare.com/trust-hub/gdpr/ (fetched 2026-08-25). Durable Objects encryption at rest (LUKS, AES-256): https://developers.cloudflare.com/durable-objects/reference/data-security/ (fetched 2026-08-25). Workers Logs retention: https://developers.cloudflare.com/workers/observability/logs/workers-logs/ (fetched 2026-08-25). |

### 2. Snowflake Inc. (Crunchy Bridge)

| | |
|---|---|
| **Status** | Active sub-processor |
| **Legal entity** | **Snowflake Inc.** (United States), the contracting party and the named processor under the Crunchy Bridge Terms of Service and Data Processing Addendum, both Last Updated June 27, 2025 (Citations below). Crunchy Data Solutions, Inc. is a United States affiliate sub-processor on the Crunchy Bridge list. |
| **Service** | Managed PostgreSQL (Crunchy Bridge) hosting both the platform's relational mirror and the identity server's database |
| **Data processed** | Fleet and device metadata, shadow mirror, telemetry history, firmware catalog, alert definitions and state, forwarding endpoints (including their bearer tokens), organizations, members, invitations, business and tax details, billing counters, contact-form submissions, error reports; the Kratos identity database (email addresses, names, phone numbers, credential hashes, sessions, verification and recovery codes, transactional-email log) |
| **Processing location** | Amazon Web Services, **us-east-1 (N. Virginia), United States**. Confirmed by the vendor in writing on 2026-09-01: the full instance, including the write-ahead-log archive and base backups, sits in us-east-1. The region is fixed at cluster creation. |
| **Transfer mechanism** | The 2021/914 SCCs, Modules Two and Three, deemed signed. The Crunchy Bridge Data Processing Addendum (Last Updated June 27, 2025) Section 11.2 applies the Standard Contractual Clauses to any Restricted Transfer from the EEA or Switzerland, and the UK Addendum to UK transfers, and states that both "shall be incorporated into the Agreement and deemed signed by the Parties". Section 1.1 defines the clauses by reference to the form published at https://www.snowflake.com/content/dam/snowflake-site/legal/legal-files/sccs.pdf. That form was read in full on 2026-09-01 and is the Annex to Commission Implementing Decision (EU) 2021/914, Clauses 1 to 18, with Modules Two (controller to processor) and Three (processor to processor) both present and pre-selected, Clause 9 taking the general authorization option with 28 days' notice of sub-processor changes, and Clauses 17 and 18 choosing the law and courts of the Netherlands, or Switzerland for Swiss transfers. No separate signature is required or available; the clauses bind through acceptance of the Terms of Service. |
| **DPF** | **Not verified.** No Crunchy Data statement found. Snowflake Inc.'s DPF notice (https://www.snowflake.com/en/legal/privacy/data-privacy-framework-notice/, fetched 2026-08-25, last updated March 10, 2025) states Snowflake Inc. is self-certified and names no subsidiaries; Crunchy Data is not mentioned. No DPF coverage is claimed for this row. |
| **Vendor sub-processors** | https://crunchybridge.com/subprocessors (last updated May 20, 2025). Thirteen entries. Infrastructure: Amazon Web Services Inc., Microsoft Corp., Google Inc., each hosting "the data you store in your specified cloud provider", so the region follows the customer's selection. Services: Google (web analytics), Zendesk (helpdesk), Stripe (payments), Peaberry Software Inc. trading as Customer.io (onboarding), Sentry Inc. (error tracking), Mezmo (log processing). Affiliates: Crunchy Data Solutions, Inc. (United States), Crunchy Data Canada Ltd (Canada), Crunchy Data Australia Pty Ltd (Australia), Crunchy Data France SAS (France). |
| **Backups** | For each cluster, a PostgreSQL base backup is captured each day and kept current by streaming the write-ahead log every 60 seconds or 16 MB, whichever comes first. The platform retains 10 days of these backups automatically. |
| **Certifications** | SOC 2 Type 2 for Crunchy Bridge ("please contact us"); AES-256 at rest; TLS 1.2 or higher required; single-tenant network isolation. https://docs.crunchybridge.com/concepts/security and https://www.crunchydata.com/security (both fetched 2026-08-25) |
| **Citations** | DPA https://www.snowflake.com/en/legal/other/crunchy-bridge/data-processing-addendum/ (Last Updated June 27, 2025); Terms of Service https://www.snowflake.com/en/legal/other/crunchy-bridge/terms-of-service/ (Last Updated June 27, 2025); sub-processors https://crunchybridge.com/subprocessors (last updated May 20, 2025); the incorporated SCC form https://www.snowflake.com/content/dam/snowflake-site/legal/legal-files/sccs.pdf; security https://docs.crunchybridge.com/concepts/security and https://www.crunchydata.com/security. All fetched 2026-09-01 except the security pages, fetched 2026-08-25. |

### 3. OVH US LLC (OVHcloud)

| | |
|---|---|
| **Status** | Active sub-processor |
| **Legal entity** | OVH US LLC dba OVHcloud, 11950 Democracy Drive, Suite 300, Reston, VA 20190 (from its DPA and EU privacy notice). This is the US subsidiary, not OVH SAS (France). |
| **Service** | Virtual private server hosting the self-hosted identity server (Ory Kratos), its administrative console, the CoAP/DTLS device-transport terminator, the MQTT device broker (Mosquitto, TLS with per-device pre-shared keys, in production since 2026-08-27), and the demo feeder |
| **Data processed** | Identity traffic in transit and in process (registration, login, recovery, settings flows: email addresses, names, credentials, session cookies); device traffic in transit through the terminator and the MQTT broker (telemetry, shadow reports, log uploads, per-device PSKs fetched from the API); the host's system journal. The identity database itself is on Crunchy Bridge, not on this host. |
| **Processing location** | **Vint Hill, Virginia, United States**. |
| **Transfer mechanism** | OVHcloud US Data Processing Agreement, last updated December 10, 2025, which incorporates the 2021 EU SCCs ("Standard Contractual Clauses for, as applicable, (a) Controller-to-Processor or (b) Processor-to-Processor Transfers approved by European Commission Decision of 4 June 2021", Schedules 1 and 2) and the UK Addendum "Version B1.0" (Schedule 3). The DPA itself does not mention the DPF. |
| **DPF** | **VERIFIED Active Participant** (listed as "OVHcloud"; dispute contact OVH US LLC, Reston VA): EU-U.S. DPF Active (orig. 2017-12-11, HR and Non-HR), UK Extension Active (orig. 2024-01-12, Non-HR only), Swiss-U.S. Active (orig. 2017-12-11, Non-HR only); all next due 2027-01-29. Verification method Outside Compliance Review. List entry verified against the official list on 2026-09-01; the SCCs in the DPA remain the primary mechanism for this row. |
| **EU representative** | OVH US LLC names an Article 27 representative in France in its EU Privacy Notice. |
| **Vendor sub-processors** | Embedded as Annex III of Schedule 1 of the DPA (affiliate and third-party entities by country); no separate URL. |
| **Certifications** | No audit report has been obtained for the Vint Hill site. |
| **Citations** | DPA: https://us.ovhcloud.com/legal/data-processing-agreement/ (fetched 2026-08-25). EU Privacy Notice: https://us.ovhcloud.com/legal/eu-privacy-notice/ (fetched 2026-08-25). |

### 4. Stripe, LLC

| | |
|---|---|
| **Status** | Active sub-processor |
| **Legal entity** | Stripe, LLC (United States). Stripe's privacy center lists "Stripe, LLC" as the contracting entity for "All activities" for United States accounts; the Provider is a US account. Stripe Payments Europe, Limited is not the Provider's contracting entity. |
| **Service** | Subscription billing: customer and subscription records, hosted Checkout, the customer billing portal, metered usage reporting, invoicing and payment collection, webhook delivery |
| **Data processed** | Organization name, billing email address and organization identifier (sent when a customer record is created); subscription identifiers and status; usage quantities (message and device counts); the customer's payment details and invoice history, which Stripe collects directly in Checkout and the portal and which never reach the Provider. Business name and tax registration number are collected by the platform but **not yet sent to Stripe** (the code marks this as an open seam). |
| **Processing location** | United States, with Stripe's own global sub-processors as listed on its service-providers page. |
| **Transfer mechanism** | Stripe Data Processing Agreement (last updated November 18, 2025), which "forms part of the Agreement", incorporates the EU SCCs (Module One controller-to-controller and Module Two controller-to-processor) and the "UK International Data Transfer Addendum", and references the Data Privacy Framework. Stripe processes some of this data as an independent controller for fraud prevention, compliance and its own legal obligations, which is why Module One appears in its DPA. |
| **DPF** | **VERIFIED Active Participant** (Stripe, LLC): EU-U.S. DPF, UK Extension and Swiss-U.S. all Active, orig. certification 2026-05-11 (a fresh certification under the LLC entity, superseding the Privacy Shield-era "Stripe Inc." confusion), next due 2027-05-11. HR and Non-HR; verification method Self-Assessment; Non-HR recourse JAMS. List entry verified against the official list on 2026-09-01. |
| **Vendor sub-processors** | https://stripe.com/legal/service-providers ("Stripe Service Providers, Sub-Processors & Affiliates", last updated December 20, 2025; fetched 2026-08-25) |
| **Certifications** | PCI DSS Level 1 service provider (widely published; not separately fetched here) |
| **Citations** | DPA: https://stripe.com/legal/dpa (fetched 2026-08-25). DPF policy: https://stripe.com/legal/data-privacy-framework (fetched 2026-08-25). Privacy center entity table: https://stripe.com/privacy-center/legal (fetched 2026-08-25). |

### 5. Resend (Plus Five Five, Inc.), not currently used

Listed for transparency only. **No data is sent to Resend today**, so it is not a sub-processor of the Service; the row records the alternative already assessed for the email rail.

| | |
|---|---|
| **Status** | Listed only, no data sent |
| **Legal entity** | Plus Five Five, Inc., 2261 Market Street #5039, San Francisco, CA 94114 (from its DPA) |
| **Transfer mechanism** | Resend Data Processing Addendum (updated December 31, 2025) incorporating the EU SCCs "approved by the European Commission in Commission Decision 2021/914 dated 4 June 2021" (Modules One, Two and Three) and the UK Addendum; Section 11.1 states "The Company complies with the EU-U.S. Data Privacy Framework (EU-U.S. DPF) and the UK Extension to the EU-U.S. DPF." |
| **DPF** | Vendor statement fetched (DPA Section 11.1; changelog post of March 13, 2025). Participant page: https://www.dataprivacyframework.gov/participant/8907. **List entry not verified.** |
| **Vendor sub-processors** | https://resend.com/legal/subprocessors (last updated 2026-07-15; 22 entries, all United States, including "Amazon Web Services, Inc. (USA): Third party hosting and sending provider"); fetched 2026-08-25 |
| **Citations** | https://resend.com/legal/dpa (fetched 2026-08-25). https://resend.com/changelog/data-privacy-framework-certification (fetched 2026-08-25). |

## Not sub-processors, but worth a line

- **VIES (European Commission VAT Information Exchange System).** When an organization saves an EU VAT number, the backend queries VIES to validate it and re-checks pending numbers hourly. The VAT number and the member-state code go to the Commission's public service. The Commission is not processing on the Provider's behalf; this is a lookup against a public register, disclosed in the privacy policy.
- **Customer-configured telemetry endpoints.** Telemetry forwarded to an endpoint the customer configures goes to whoever the customer chose; that recipient is the customer's processor, not the Provider's.
- **Amazon Web Services.** Reaches customer data only as a sub-processor of the database provider (hosting). Listed on that list, not this one.
- **Ory.** Kratos is self-hosted; no Ory-operated service receives data.
- **GreptimeDB.** Retired from production and staging; only the local development stack uses it.

## Summary table

| Entity | Status | Service | Location | Mechanism | Verified | Citation | Fetched |
|---|---|---|---|---|---|---|---|
| Cloudflare, Inc. | Active | Edge platform, storage, queues, email, logs | Global network; US and EU in normal use; no jurisdiction pinning | Cloudflare DPA v6.4 (SCCs Module 2, UK Addendum); vendor states DPF | DPA yes; DPF list verified 2026-09-01 | https://www.cloudflare.com/cloudflare-customer-dpa/ | 2026-08-25 |
| Snowflake Inc. (Crunchy Bridge) | Active | Managed PostgreSQL | AWS us-east-1, N. Virginia, US (vendor-confirmed in writing, incl. WAL and base backups) | Crunchy Bridge DPA of June 27, 2025, incorporating the 2021/914 SCCs Modules Two and Three, deemed signed | DPA yes, read in full; DPF not claimed | https://www.snowflake.com/en/legal/other/crunchy-bridge/data-processing-addendum/ | 2026-09-01 |
| OVH US LLC | Active | VPS (identity server, device terminator, MQTT broker) | Vint Hill, Virginia, US | OVHcloud US DPA (2021 SCCs, UK Addendum B1.0); vendor states DPF | DPA yes; DPF list verified 2026-09-01 | https://us.ovhcloud.com/legal/data-processing-agreement/ | 2026-08-25 |
| Stripe, LLC | Active | Billing and payments | US | Stripe DPA (SCCs Modules 1 and 2, UK Addendum); vendor states DPF | DPA yes; DPF list verified 2026-09-01 | https://stripe.com/legal/dpa | 2026-08-25 |
| Plus Five Five, Inc. (Resend) | Listed only, no data sent | Outbound email (candidate) | US | Resend DPA (SCCs Modules 1 to 3, UK Addendum); vendor states DPF | DPA yes; DPF list no | https://resend.com/legal/dpa | 2026-08-25 |

## Change log

Changes to this list are announced under Section 6 of the DPA. This is the first published version; its date is the one at the top of this page.
