# Data Processing Agreement

Last updated: {{LAST_UPDATED}}

This Data Processing Agreement ("DPA") forms part of the PidgeIoT Terms of Service (the "Agreement") between Justin's Engineering Services LLC, a Montana limited liability company registered as a foreign limited liability company in Massachusetts, with its principal office at 30 Virginia Ave, West Springfield, MA 01089 ("Provider", "Processor", "we"), and the customer identified through the applicable account or organization record or, where applicable, a countersigned copy of this DPA ("Customer", "Controller", "you").

"Service" means the PidgeIoT service provided under the Agreement.

This DPA applies to the processing of Customer Personal Data where that processing is subject to the GDPR, UK GDPR or FADP. Sections 4, 5, 6, 7, 8, 10, 11 and 12 also apply as contractual commitments concerning Customer Personal Data regardless of whether those laws apply, except where a provision expressly states otherwise.

---

## 1. Definitions

- "Data Protection Law" means the EU General Data Protection Regulation (EU) 2016/679 ("GDPR"), the UK GDPR and the UK Data Protection Act 2018, the Swiss Federal Act on Data Protection ("FADP"), and any law implementing or supplementing them, in each case to the extent applicable to the processing under this DPA.
- "Personal Data", "Controller", "Processor", "Data Subject", "Processing", "Supervisory Authority" and "Personal Data Breach" have the meanings given under the applicable Data Protection Law.
- "Customer Personal Data" means Personal Data that the Customer, its users or its devices submit to the Service and that the Provider processes on the Customer's behalf. The categories are described in Annex I.
- "Sub-processor" means a third party engaged by the Provider to process Customer Personal Data.
- "EU SCCs" means the standard contractual clauses for the transfer of personal data to third countries adopted by the European Commission in Commission Implementing Decision (EU) 2021/914 of 4 June 2021.
- "UK Addendum" means the International Data Transfer Addendum to the EU Commission Standard Contractual Clauses issued by the UK Information Commissioner under section 119A of the Data Protection Act 2018, Version B1.0, in force from 21 March 2022.
- "Restricted Transfer" means a transfer of Customer Personal Data subject to applicable Data Protection Law to a country or recipient not covered by an applicable adequacy decision or other lawful transfer basis under that Data Protection Law.
- "Technical and Organizational Measures" or "TOMs" means the measures described in Annex II.

## 2. Roles and scope

2.1. For Customer Personal Data, the Customer is the Controller and the Provider is the Processor. Where the Customer itself acts as a processor for a third-party controller, the Customer warrants that its instructions to the Provider are authorized by that controller, and the Provider acts as the Customer's sub-processor.

2.2. This DPA applies only to Customer Personal Data. Personal Data for which the Provider acts as Controller is not Customer Personal Data and is governed by the PidgeIoT Privacy Policy. Provider-controlled Personal Data includes:

(a) account identity and authentication information used by the Provider to create, secure and administer individual user accounts;

(b) billing and tax information processed by the Provider for billing, payment administration and legal compliance;

(c) contact-form submissions, feedback and other communications sent directly to the Provider;

(d) identified diagnostics and support information submitted to or collected by the Provider for its own support, security and service-administration purposes;

(e) infrastructure and request logs processed by the Provider for its own security, administration and operational purposes; and

(f) records evidencing acceptance of the Provider's Terms, Privacy Policy, DPA or other Provider contractual terms or notices.

Customer Personal Data is described in Annex I and includes Personal Data that the Provider processes on the Customer's behalf through the Customer's use and configuration of the Service.

2.3. Each party will comply with its own obligations under Data Protection Law. The Customer is responsible for the lawfulness of the Personal Data it, its users and its devices send to the Service, including providing any required notices and establishing any required legal basis for that processing.

The Customer is also responsible for configuring the Service, including devices, telemetry keys, alert channels and forwarding endpoints, so that it does not send Personal Data that the Customer is not authorized to process, and for keeping Customer-controlled credentials, device credentials and pre-shared keys confidential and protected against unauthorized use.

## 3. Subject matter, duration, nature and purpose

3.1. **Subject matter.** The processing of Customer Personal Data that occurs when the Customer, its users or its devices use the Service to provision, authenticate, configure, monitor, update, alert on and diagnose IoT devices, and to manage related users, access, notifications and Service functionality.

3.2. **Duration.** For the term of the Agreement and until Customer Personal Data has been deleted or returned under Section 8.

3.3. **Nature.** Collection from the Customer's devices and users; storage; retrieval and display; transmission to endpoints the Customer configures, including alert notifications and forwarded telemetry; distribution of firmware and related binaries to devices; relay of diagnostic commands to devices where enabled; and deletion. The processing is automated and hosted.

3.4. **Purpose.** The Provider processes Customer Personal Data to provide the Service as described in the Agreement and published API documentation and as further instructed by the Customer through its use and configuration of the Service, including instructions submitted by devices under their own credentials.

Features provided to a Customer may analyze that Customer's Customer Personal Data, including through automated or machine-learning methods, where that analysis is performed to provide the Service to that Customer.

The Provider may derive aggregated statistics about use of the Service, such as account, device and message counts, that do not identify and cannot reasonably be used to identify any Customer, device or person. The Provider may use those aggregated statistics to operate, secure, improve and describe the Service, but does not use the contents of Customer Personal Data for that purpose and will not disclose the statistics in a form that identifies the Customer.

The Provider does not sell Customer Personal Data, use Customer Personal Data to build advertising or cross-customer profiles, or use Customer Personal Data to train models across customers. Any broader use of Customer Personal Data applies only where the Customer affirmatively enrolls in that program under separate terms.

3.5. The categories of Personal Data and of Data Subjects are set out in Annex I.

## 4. Processor obligations

The Provider will:

4.1. **Documented instructions (Art. 28(3)(a)).** Process Customer Personal Data only on the Customer's documented instructions, including with regard to a Restricted Transfer, unless required to do so by applicable law to which the Provider is subject. In that case, the Provider will inform the Customer of that legal requirement before processing unless the law prohibits notice on important grounds of public interest.

The Agreement, this DPA, the Customer's configuration and use of the Service, API calls made under the Customer's credentials, instructions submitted by devices under their own credentials, and any affirmative enrollment by the Customer in a separate program under Section 3.4 constitute the Customer's documented instructions.

The Provider will inform the Customer without undue delay if, in its opinion, an instruction infringes applicable Data Protection Law. The Provider is not required to perform a general legal review of the Customer's instructions.

4.2. **Confidentiality (Art. 28(3)(b)).** Ensure that every person authorized to process Customer Personal Data is bound by a contractual or statutory duty of confidentiality. The Provider will ensure that any employee, contractor, agent or other person granted access to Customer Personal Data is subject to that confidentiality obligation before access is granted.

4.3. **Security (Art. 28(3)(c), Art. 32).** Implement and maintain the TOMs in Annex II, taking into account the state of the art, the costs of implementation, the nature, scope, context and purposes of the processing, and the risk to Data Subjects. The Provider may update the TOMs from time to time to reflect changes in the Service, law, technology or risk, provided that the overall level of protection for Customer Personal Data is not materially reduced.

4.4. **Sub-processors (Art. 28(2) and (4)).** Engage Sub-processors only under Section 6.

4.5. **Assistance with Data Subject requests (Art. 28(3)(e)).** Taking into account the nature of the processing, assist the Customer by appropriate technical and organizational measures, insofar as possible, in fulfilling the Customer's obligation to respond to requests to exercise Data Subject rights concerning Customer Personal Data.

Available self-service functions relevant to Customer Personal Data include deletion of devices, fleets that hold no devices, and organizations that own no fleets through the dashboard, and export of authorized Service data through documented API routes where available. Self-service rights and request procedures concerning Personal Data for which the Provider acts as Controller are described separately in the PidgeIoT Privacy Policy.

Where a request concerning Customer Personal Data cannot be satisfied through self-service, the Provider will act on the Customer's written request within ten (10) business days, or sooner where the Customer's applicable statutory deadline reasonably requires it.

If a Data Subject contacts the Provider directly about Customer Personal Data, the Provider will not respond on the merits unless required by law, but will forward the request to the Customer within five (5) business days.

4.6. **Assistance with security, breach notification and DPIAs (Art. 28(3)(f), Arts. 32 to 36).** Taking into account the nature of the processing and the information available to the Provider, assist the Customer in ensuring compliance with its obligations under Articles 32 to 36 GDPR or equivalent applicable Data Protection Law, including by making available the information in Annex II and the published Sub-processor list for use in data protection impact assessments and prior consultations.

Reasonable assistance necessary for the Provider to satisfy its mandatory obligations under applicable Data Protection Law or the EU SCCs is included in the Service. If the Customer requests substantial additional assistance beyond those obligations and beyond information ordinarily made available by the Provider, the parties may agree in writing in advance on reasonable fees for that additional assistance.

4.7. **Deletion or return (Art. 28(3)(g)).** At the Customer's choice, delete or return Customer Personal Data at the end of the provision of the Service under Section 8.

4.8. **Demonstrating compliance and audits (Art. 28(3)(h)).** Make available to the Customer all information necessary to demonstrate compliance with Article 28 GDPR and allow for and contribute to audits under Section 7.

4.9. **Records.** Maintain the records of processing activities required of a processor under Article 30(2) GDPR or equivalent applicable Data Protection Law to the extent required, and make those records available to the competent Supervisory Authority on request where required by applicable law.

## 5. Personal Data Breach

5.1. The Provider will notify the Customer without undue delay and in any case within forty-eight (48) hours after becoming aware of a Personal Data Breach affecting Customer Personal Data. Notice will be sent to the email address of each organization owner on the Customer's account, to any security contact address the Customer has provided, or, where the Customer is not associated with an organization, to the email address on the Customer's account. The Provider's security contact is security@pidgeiot.com.

5.2. The notification will describe, to the extent known at the time and supplemented as information becomes available: the nature of the breach, the categories and approximate number of Data Subjects and records concerned, the likely consequences, the measures taken or proposed to address it, and a contact point.

5.3. The Provider will document each Personal Data Breach affecting Customer Personal Data, including the facts relating to the breach, its effects and the remedial action taken, and will make that documentation available where required under applicable Data Protection Law or the EU SCCs.

5.4. The Provider's notification of, or response to, a Personal Data Breach is not an acknowledgement of fault or liability.

5.5. The Provider will not notify a Supervisory Authority or Data Subjects on the Customer's behalf unless the Customer instructs it to in writing or the law requires it.

## 6. Sub-processors

6.1. **General authorization.** The Customer gives the Provider general written authorization to engage the Sub-processors identified as currently processing Customer Personal Data on the published Sub-processor list at [https://pidgeiot.com/subprocessors/](https://pidgeiot.com/subprocessors/) and any replacement or additional Sub-processor appointed in accordance with this Section. This constitutes general written authorization for purposes of Article 28(2) GDPR and Option 2 under Clause 9(a) of the EU SCCs.

A vendor identified on the published list as not currently receiving Customer Personal Data is not treated as an active Sub-processor solely because it appears on the list. If the Provider later begins sending Customer Personal Data to that vendor, Section 6.2 applies before that processing begins.

6.2. **Notice of changes.** The Provider will publish changes to the Sub-processor list and, where reasonably practicable, will give the Customer at least thirty (30) days' prior notice before a new or replacement Sub-processor begins processing Customer Personal Data. Notice will be sent to the Customer's organization owners or, if the Customer is not associated with an organization, to the email address on the Customer's account.

If a Sub-processor change is initiated by an existing vendor on less than thirty (30) days' notice, or if a replacement is urgently necessary to protect the security or availability of the Service, the Provider may proceed on shorter notice and will notify the Customer as soon as reasonably practicable. The Customer's objection rights under Section 6.3 will continue to apply.

6.3. **Objection.** The Customer may object in writing during the applicable notice period on reasonable data-protection grounds. The parties will work in good faith to resolve the objection. If the objection cannot reasonably be resolved, the Customer may terminate the affected part of the Service or, if the Sub-processor is necessary to provide the Service, the Agreement, without penalty.

If the Customer has prepaid fees for a period extending beyond the effective date of termination, the Provider will refund the unused portion of those prepaid fees. No refund is due for amounts already earned or for usage or services already provided.

6.4. **Flow-down.** The Provider will enter into a written agreement with each Sub-processor imposing data-protection obligations appropriate to the processing and sufficient to meet the requirements of applicable Data Protection Law, including Article 28(4) GDPR where applicable. The Provider remains fully liable to the Customer for the performance of each Sub-processor's applicable data-protection obligations to the extent required by Article 28(4) GDPR.

Where required by the EU SCCs, the Provider will, on the Customer's request, provide a copy of the relevant Sub-processor agreement, which may be redacted as necessary to protect commercial secrets, confidential information, Personal Data and other information not relevant to demonstrating compliance.

6.5. **Emergency replacement.** Any urgent Sub-processor replacement will be handled under Section 6.2, including prompt notice and the Customer's continuing objection rights under Section 6.3.

## 7. Audits

7.1. **Documentation first.** The Provider will answer the Customer's reasonable written security and privacy questionnaires and will provide on request: this DPA and its Annexes; the current published Sub-processor list, including the security or certification information the Provider publishes or relies on for those Sub-processors; the published architecture and API documentation; and information describing any material change to the TOMs that affects the protection of Customer Personal Data.

The Provider will respond within thirty (30) days of a written request and not more than once in any twelve-month period unless a Personal Data Breach or a Supervisory Authority requires otherwise.

7.2. **On-site or remote inspection on cause.** Where the information provided under Section 7.1 is insufficient to demonstrate compliance and the Customer has a reasonable, documented basis to believe the Provider is not complying with this DPA, or where a Supervisory Authority requires an audit, the Customer or an independent auditor bound by confidentiality and reasonably acceptable to the Provider may conduct an audit on at least thirty (30) days' written notice, during business hours, no more than once in any twelve-month period unless a Personal Data Breach or Supervisory Authority requires otherwise.

The audit must be limited to the processing governed by this DPA and must not provide access to other customers' data or to Sub-processors' facilities except to the extent legally required. The Customer will bear its own audit costs.

Reasonable Provider assistance required to satisfy mandatory obligations under applicable Data Protection Law or the EU SCCs is included. For substantial additional Provider time requested by the Customer beyond those obligations, the Customer will reimburse the Provider at the rate and above any no-charge threshold stated in the applicable order form or other written agreement between the parties. If no such rate or threshold has been agreed, the parties will agree them in writing before chargeable work is performed. No fee will apply where the audit identifies a material breach of this DPA by the Provider.

7.3. Audit findings are confidential information of both parties. The Provider will remediate confirmed material non-compliance within a reasonable time agreed with the Customer.

7.4. Nothing in this Section limits the audit rights that Clause 8.9 of the EU SCCs grants where they apply.

## 8. Deletion and return

8.1. **During the term.** The Customer can delete devices, fleets that hold no devices, and organizations that own no fleets through the dashboard and API where those functions are available. Deleting a device removes the corresponding device data, including stored telemetry history associated with that device, subject to any temporary or residual copies described in Annex II. Firmware and related binaries remain stored until removed at the Customer's request or until deletion is otherwise required under this DPA.

Telemetry history is retained according to the service tier at which the relevant account or organization is being served: 7 days while served at free-tier limits, 30 days on Builder, 90 days on Growth, and 13 months on Scale and Fleet. A suspended paid account or organization is subject to free-tier retention limits. Where telemetry is forwarded to a Customer-configured endpoint without stored history being created, no telemetry-history retention period applies to that forwarded data.

8.2. **At the end of the Service.** Upon termination or expiration of the Agreement, the Customer may request return of Customer Personal Data still held by the Provider. Where requested and reasonably available, the Provider will return Customer Personal Data in a machine-readable format, which may include JSON exports and raw stored binaries for firmware images or device-log data.

Unless the Customer requests return during the applicable termination process, the Provider will delete Customer Personal Data within thirty (30) days after termination or expiration, except to the extent retention is required by applicable law or the Privacy Policy expressly provides for continued retention of Provider-controlled Personal Data. Copies contained in backups will expire under the database provider's normal backup-rotation schedule and will remain subject to the confidentiality and security obligations of this DPA until they expire.

8.3. **Legal retention.** The Provider may retain Customer Personal Data to the extent and for the period required by applicable law. Any Customer Personal Data retained under this Section will be processed only for the legally required purpose and will remain subject to the confidentiality and security obligations of this DPA.

8.4. **Deletion procedure.** Deletion will be carried out using the applicable account, organization, fleet and device deletion procedures described in Annex II. The Provider will confirm completion in writing on reasonable request.

## 9. International transfers

9.1. **Where processing happens.** The Provider is established in the United States. Customer Personal Data may be processed in the United States and through the infrastructure and global network of the Provider's Sub-processors as described in Annexes I through III and the published Sub-processor list. Device state may be created and stored in the region selected by the edge infrastructure based on where the device or account is first established, and some device traffic, including CoAP and MQTT traffic, may terminate on the Provider's United States-based server infrastructure before being processed further.

A transfer of Customer Personal Data subject to the GDPR, UK GDPR or FADP to the Provider or another recipient outside the applicable protected jurisdiction will be treated as a Restricted Transfer unless covered by an applicable adequacy decision or other lawful transfer mechanism.

The Provider is not currently certified under the EU-U.S. Data Privacy Framework and does not rely on that framework for transfers to itself.

9.2. **EU SCCs incorporated by reference.** For Restricted Transfers subject to the GDPR, the parties enter into the EU SCCs, Module Two (Transfer controller to processor), which are incorporated into this DPA by reference and take effect on the date the Customer accepts this DPA, with the following selections:

- Clause 7 (Docking clause): included.
- Clause 9(a) (Use of Sub-processors): Option 2, general written authorization, with the notice period in Section 6.2.
- Clause 11(a) (Redress): the optional independent dispute-resolution language is not included.
- Clause 13 (Supervision): the Supervisory Authority determined under Annex I, Part C.
- Clause 17 (Governing law): Option 1, the law of Ireland.
- Clause 18(b) (Choice of forum): the courts of Ireland.
- Annex I of the EU SCCs is completed by Annex I of this DPA; Annex II of the EU SCCs by Annex II of this DPA; Annex III of the EU SCCs by Annex III of this DPA.

Where the Customer acts as a processor for a third-party controller, Module Three (Transfer processor to processor) applies instead, with the same selections to the extent compatible with that Module.

9.3. **UK transfers.** For Restricted Transfers subject to the UK GDPR, the EU SCCs as completed above apply as amended by the UK Addendum, which is incorporated by reference. For the purposes of the UK Addendum: Table 1 (Parties) is completed by Annex I, Part A; Table 2 (Selected SCCs, Modules and Selected Clauses) refers to the EU SCCs with the selections in Section 9.2; Table 3 (Appendix Information) refers to Annexes I, II and III of this DPA; for Table 4 of the UK Addendum, neither party may terminate the Addendum solely because the Approved Addendum changes, unless the UK Addendum or applicable law requires otherwise.

9.4. **Swiss transfers.** For Restricted Transfers subject to the FADP, the EU SCCs as completed under Section 9.2 apply with the adaptations required under Swiss data protection law. References to the GDPR will be read, where appropriate, as references to the corresponding provisions of the FADP; the competent authority for Swiss processing is the Swiss Federal Data Protection and Information Commissioner (FDPIC); and references to a "Member State" will not be interpreted to prevent Data Subjects in Switzerland from exercising their rights in Switzerland.

9.5. **Sub-processor transfers.** Onward transfers to Sub-processors will be covered by the transfer mechanism identified for that Sub-processor in the published Sub-processor list or otherwise required by applicable Data Protection Law. Where a Sub-processor participates in an applicable adequacy or certification framework that lawfully covers the transfer, the Provider may rely on that mechanism for the onward transfer.

9.6. **Supplementary measures and government access.** In support of the parties' assessment of Restricted Transfers, the parties acknowledge that Customer Personal Data is protected in transit and at rest as described in Annex II; device authentication uses per-device credentials and, where applicable, pre-shared keys under the controls described in Annex II; diagnostics are handled in accordance with the de-identification and retention measures described in the Privacy Policy and Annex II; and access to Customer Personal Data is limited in accordance with the technical and organizational measures described in this DPA.

As of the Effective Date, the Provider represents that it has not received a legally binding request from a public authority for access to Customer Personal Data and is not subject to an order prohibiting disclosure of any such request, except to the extent separately disclosed to the Customer.

Unless legally prohibited, the Provider will notify the Customer of a legally binding request from a public authority for access to Customer Personal Data, will challenge the request where there are reasonable grounds to do so, and will disclose only the minimum information legally required.

Where applicable law or the EU SCCs require the parties to document a transfer impact assessment or equivalent assessment, the parties will cooperate in good faith to maintain documentation reasonably necessary for that purpose.

9.7. **Conflict.** If the EU SCCs, the UK Addendum, or the Swiss adaptations conflict with this DPA or the Agreement, the transfer terms prevail for the transfer they govern.

9.8. **Alternative mechanism.** If a Supervisory Authority, court or applicable law determines that a transfer mechanism used under this Section is invalid or insufficient, the parties will cooperate in good faith to implement another lawful transfer mechanism where reasonably available.

If no lawful transfer mechanism is reasonably available, the Provider may suspend the affected transfer or affected processing to the extent necessary to comply with applicable law. If that suspension materially prevents the Provider from supplying the affected part of the Service, either party may terminate that affected part of the Service without penalty.

## 10. Liability

The exclusions and limitations of liability in the Agreement apply to this DPA to the extent applicable. The Provider's aggregate liability arising out of or relating to this DPA and the Agreement is subject to the Provider liability cap stated in the Agreement, applied to the DPA and the Agreement together as a single cap and not separately.

Nothing in this DPA limits liability for fees owed, either party's gross negligence, fraud or willful misconduct, or any other liability that cannot lawfully be limited.

Nothing in this Section limits or alters the rights of Data Subjects under Article 82 GDPR or Clause 12 of the EU SCCs, or any other liability that the parties may not limit under applicable Data Protection Law or the EU SCCs.

As between the parties, any allocation of responsibility or contribution for amounts paid to a Data Subject will be determined in accordance with each party's responsibility for the event giving rise to the claim and applicable law, without limiting the Data Subject's rights against either party.

## 11. Term and termination

This DPA takes effect when the Customer accepts the Agreement incorporating it, countersigns a copy of this DPA, or continues to use the Service after receiving notice that this DPA or an applicable update to this DPA applies to the Customer.

This DPA continues for as long as the Provider processes Customer Personal Data on the Customer's behalf and until that Customer Personal Data has been deleted or returned under Section 8.

Sections 4.2, 4.3, 5, 8, 9, 10 and 12, and any other provision that by its nature is intended to survive termination, will survive for so long as necessary to give effect to their terms.

## 12. Precedence, changes, and miscellaneous

12.1. **Precedence.** If there is a conflict among the documents governing the Service:

(a) the EU SCCs, UK Addendum and applicable Swiss transfer terms control for the Restricted Transfers they govern;

(b) this DPA controls for matters concerning the processing of Personal Data;

(c) a signed order form or other written agreement between the parties that expressly references the Agreement controls for the matters it addresses, except that it does not change the governing law or forum of this DPA under Section 12.3, does not override the transfer terms described in subsection (a), and does not amend this DPA with respect to Personal Data unless it expressly states that it amends this DPA; and

(d) otherwise, the order of precedence stated in the Agreement applies.

12.2. **Changes.** The Provider may update this DPA to reflect changes in law, the Service, Sub-processors, security measures or processing practices. Where an update materially affects the protection of Customer Personal Data or the Customer's rights under this DPA, the Provider will give at least thirty (30) days' prior notice by email to the Customer's organization owners or, if the Customer is not associated with an organization, to the email address on the Customer's account, and will update the published version.

Where an urgent change is reasonably necessary to address a security issue, legal requirement or urgent Sub-processor replacement, the Provider may implement the change on shorter notice and will notify the Customer as soon as reasonably practicable.

The Provider may update Annex II from time to time to reflect changes in the Service, technology and security measures, provided that the overall level of protection for Customer Personal Data is not materially reduced. A material reduction in protection is subject to the notice requirement above.

If a notified change materially reduces the protection of Customer Personal Data and the parties cannot reasonably resolve the Customer's objection before the change takes effect, the Customer may terminate the affected part of the Service without penalty.

12.3. **Governing law and forum.** Except for the EU SCCs, UK Addendum and Swiss transfer provisions, which are governed by the law and forum specified in those instruments, this DPA and the parties' rights and obligations under it are governed by the laws of the Commonwealth of Massachusetts, without regard to conflict-of-law principles.

Any action arising out of or relating to this DPA that is not required to be brought in another forum under the applicable transfer terms will be brought in the state courts located in Hampden County, Massachusetts, or, where federal jurisdiction exists, the United States District Court for the District of Massachusetts, and each party consents to that jurisdiction and venue.

12.4. **Notices.** Notices under this DPA to the Provider will be sent to **privacy@pidgeiot.com**. Security notices to the Provider may also be sent to **security@pidgeiot.com**. Notices to the Customer will be sent to the Customer's organization owners or, if the Customer is not associated with an organization, to the email address on the Customer's account, together with any security or legal contact address the Customer has provided.

---

## Signature block

**Data Importer / Provider**\
Justin's Engineering Services LLC, a Montana limited liability company registered as a foreign limited liability company in Massachusetts\
Contact: Justin Forgue, Member\
30 Virginia Ave\
West Springfield, MA 01089\
Email: privacy@pidgeiot.com\
Signature: ______________________________\
Date: __________________________________

**Data Exporter / Customer**\
Customer legal name or individual name: __________________________\
Organization name, if applicable: ________________________________\
Contact person: _______________________________________________\
Account/contact email: _________________________________________\
Signatory name, if different: __________________________________\
Title, if applicable: __________________________________________\
Signature: ___________________________________________________\
Date: ________________________________________________________

**Effective Date**: The date on which this DPA becomes effective under Section 11. If the parties countersign this DPA, the Effective Date is the later signature date unless another effective date is expressly stated here: __________________.

---

## Annex I: Description of the processing

Completes Annex I of the EU SCCs and Tables 1 and 3 of the UK Addendum.

### Part A: List of parties

**Data exporter:** The Customer identified through the applicable account or organization record in the Service or, where applicable, in the signature block of this DPA. The Customer may be an individual or a legal entity. Where an organization record exists, identifying information may include the organization name, organization-owner contact information and other account information used to identify the Customer. Where no organization record exists, the Customer is identified by the applicable account information.

**Role:** Controller or Processor, as applicable.\
**Activities:** Use of the Service to operate, manage and monitor the Customer's devices and related data.

**Data importer:** Justin's Engineering Services LLC, a Montana limited liability company registered as a foreign limited liability company in Massachusetts.\
**Contact:** Justin Forgue, Member\
**Address:** 30 Virginia Ave, West Springfield, MA 01089\
**Email:** privacy@pidgeiot.com\
**Role:** Processor or Sub-processor, as applicable.\
**Activities:** Hosting and operating the PidgeIoT IoT platform and processing Customer Personal Data as described in this DPA.

### Part B: Description of the transfer and processing

#### Categories of Data Subjects

1. Customer users and personnel whose Personal Data appears in organization membership, device-access, alert-recipient or other Customer-controlled Service records.
2. Individuals whose Personal Data is reported by the Customer's devices or otherwise included in Customer-controlled device data. The Service is designed primarily for machine and device data, but Customer-configured telemetry may contain Personal Data depending on what the Customer chooses to send.
3. Individuals designated by the Customer to receive Service notifications or alerts.

#### Categories of Personal Data

| Category | Concrete data | Where it lives |
|---|---|---|
| **Organization and membership** | Organization name; member list and roles; member email addresses; inviting member; join date; pending invitations and related status; organization-owner contact information; access relationships and related Service records. | PostgreSQL and related Service records. |
| **Device metadata** | Device and fleet names; connector settings and endpoints; access-control identifiers for authorized users and organizations; device status and related configuration metadata. | Per-device storage objects and PostgreSQL mirror. |
| **Device shadow state** | Target configuration written from the dashboard or API and current configuration reported by the device, including firmware assignment. | Per-device storage object and PostgreSQL mirror. |
| **Telemetry** | Latest value per key and, where the Service stores history for the applicable configuration, time-series history of reported values. Telemetry may contain Personal Data depending on the keys and values chosen by the Customer. Where telemetry is forwarded directly to a Customer-configured endpoint without stored history being created, no historical telemetry copy is retained by the Provider for that forwarded data. | Per-device storage objects, PostgreSQL where history is retained, and transiently through the applicable queue or transport infrastructure. |
| **Device logs** | Up to the newest 200 chunks of device log data per device and, where provided, a Customer-uploaded log dictionary. | Per-device storage object; uploaded log dictionaries may also be stored in object storage. |
| **Firmware and Related Binaries** | Signed firmware images and related binaries uploaded by the Customer, together with version and hash metadata. Firmware storage is intended for device firmware and related binaries rather than account or telemetry data and ordinarily does not contain Personal Data. Firmware remains Customer data subject to return or deletion under Section 8 whether or not it contains Personal Data. | Object storage for binaries and PostgreSQL for related catalog metadata |
| **Alerts** | Alert name; condition; severity; notification channel; recipient email addresses; operator note or other Customer-supplied alert content; and per-device fired state. | PostgreSQL and applicable notification-delivery infrastructure. |
| **Forwarding endpoints** | URL, bearer token and related configuration for any telemetry endpoint the Customer configures. | PostgreSQL; sensitive credential values are not returned through ordinary read interfaces. |
| **Saved dashboard state** | Customer-configured saved graphs, dashboard preferences and related visualization state associated with the Customer's Service use. | Service database. |

#### Sensitive data

The Service is not designed or authorized for special-category Personal Data under Article 9 GDPR, criminal-offence data under Article 10 GDPR, or equivalent sensitive categories under other applicable Data Protection Law. The Customer must not submit such data through the Service.

#### Frequency of the transfer

Continuous, for as long as the Customer's devices and users interact with the Service.

#### Nature and purpose

See Sections 3.3 and 3.4 of the DPA.

#### Retention

Customer Personal Data is retained for the term of the Agreement and for the deletion or return period described in Section 8, subject to shorter automatic retention periods that apply to particular data categories.

Telemetry history is retained according to the service tier at which the relevant account or organization is being served: 7 days while served at free-tier limits, 30 days on Builder, 90 days on Growth, and 13 months on Scale and Fleet. Device log buffers retain the newest 200 chunks per device. Firmware and related binaries are retained until removed at the Customer's request or otherwise deleted under Section 8. Other category-specific retention periods are described in Annex II and the Privacy Policy.

#### Sub-processor transfers

The subject matter, nature and duration of processing by each Sub-processor are stated in the published sub-processor list (Annex III).

### Part C: Competent Supervisory Authority

For transfers subject to the GDPR, the competent Supervisory Authority will be determined in accordance with Clause 13 of the EU SCCs.

Where the Customer is established in the EEA, the competent authority is the Supervisory Authority of the applicable Member State determined under Clause 13.

Where the Customer is not established in the EEA but is subject to the GDPR under Article 3(2), the competent authority is the Supervisory Authority of the Member State in which the Customer's Article 27 representative is established or, if no representative has been appointed where one is not required or has not yet been appointed, the Supervisory Authority of a Member State in which affected Data Subjects are located, as provided by Clause 13.

For transfers subject to the UK GDPR, the competent authority is the UK Information Commissioner's Office.

For transfers subject to the FADP, the competent authority is the Swiss Federal Data Protection and Information Commissioner.

Where a countersigned copy of this DPA identifies a specific competent authority, that identification applies to that Customer unless inconsistent with applicable law.

---

## Annex II: Technical and Organizational Measures

This Annex describes the technical and organizational measures implemented by the Provider for Customer Personal Data and other Personal Data processed in connection with the Service. Where a measure depends on a Sub-processor's infrastructure or controls, Section H identifies the relevant reliance.

### A. Access control to systems and data

1. **Dashboard authentication.** Dashboard authentication is provided through a self-hosted Ory Kratos identity service. Supported authentication methods include password authentication, passkeys (WebAuthn), TOTP authenticator applications and one-time lookup secrets, depending on the user's enrollment and configuration. Passwords are stored using server-side hashing. Sessions last four hours. Session cookies are HttpOnly and scoped to the Service domain. A browser request that no longer resolves to a valid session is treated as unauthenticated.
2. **Authorization.** Authorization is enforced server-side on every request. The edge router resolves the session to a user identifier and the caller's server-derived organization roles and forwards that information to the device store; nothing in a request body can assert either identity or role. Organizations have three roles (owner, admin, member) with a documented per-route permission matrix; every organization must retain at least one owner. Each device carries its own access-control list inside its own storage object and refuses any user not on it. Cross-device queries in the relational mirror carry the owner or membership predicate in the query itself.
3. **Device authentication.** Each device is provisioned with its own Ed25519 key pair. The device private key is used to sign the applicable device credential and is not retained by the Provider after provisioning; the Provider stores the corresponding public key used to verify the device's credential. Rotating a device credential generates a new key pair and revokes prior credentials.

Devices using CoAP or MQTT also use a per-device pre-shared key for the applicable transport authentication. Access to that key through the internal lookup used by the transport terminator is restricted by source address and service credential. MQTT communications use TLS. CoAP communications use DTLS 1.2 with connection identifiers over UDP or the applicable protected TCP transport. Credential rotation replaces the applicable device key pair and, for a CoAP or MQTT device, its pre-shared key.

4. **Administrative access.** The identity server's administrative API binds to loopback on its host and is never published. The administrative console reaches it only on the same host, and is reachable from outside only through a Cloudflare Access application with a single-person allow-list and a 24-hour session. Cloudflare Access may also be enabled for staging deployments where configured.
5. **Host access.** SSH to the VPS is by public key only; password and keyboard-interactive authentication are disabled, root login is key-only, and repeated failed connections are banned automatically. The host firewall drops inbound traffic except SSH and the device transport ports, including port 5684 for CoAP and port 8883 for MQTT over TLS. The identity server is reachable only through an outbound Cloudflare Tunnel. Credential-bearing configuration is root-owned, with access restricted to the applicable service account.
6. **Secrets handling.** Worker secrets are set with the deployment tool's secret command and never appear in configuration files or the repository. Credential values are never printed, logged, committed or pasted into chat; they are referred to by name and read from the environment. Invite tokens are stored only as a hash. Forwarding-endpoint bearer tokens and device credentials, including applicable device pre-shared keys, are not returned through ordinary read APIs after issuance.

### B. Transmission control (encryption in transit)

1. Browser and API traffic terminates on the edge provider over TLS. Traffic from the edge to the managed PostgreSQL instance uses TLS (StartTLS through the connection pooler); the database requires TLS 1.2 or higher. Traffic from the edge to the identity server crosses a Cloudflare Tunnel.
2. Device traffic uses encrypted transport appropriate to the configured protocol, including TLS over HTTPS and WebSocket, MQTT over TLS, and the applicable protected CoAP transport. Device authentication is handled as described in Section A.3.
3. Internal calls between the edge router and each device's storage object stay inside the edge provider's network and are encrypted by that provider.

### C. Storage control (encryption at rest) and isolation

1. Per-device state (configuration, latest telemetry, logs, access list, device public key and other device credential material as applicable) is held in an isolated storage object per device; a request to one device's object cannot read another's. The edge provider encrypts that storage at rest (AES-256, provider-managed keys).
2. The relational mirror and the identity database are on a single-tenant managed PostgreSQL instance with AES-256 encryption at rest and a network isolated from other customers.
3. Firmware images are stored in a private object-storage bucket accessible only through the edge router, which checks the device's own credential before serving a byte range.
4. Environment separation. Production and staging use separate application configuration and separate databases and storage resources as applicable. The staging and production databases are logically separate databases on the same managed database instance. Staging may authenticate users through the production identity service.
5. Telemetry that the Customer forwards to its own endpoint is sent there and not retained as history by the Provider; the endpoint is the Customer's responsibility.

### D. Input control and minimization

1. Every write is attributable to a session identity, device credential or service principal. Organization membership records the applicable invitation and membership information. A device's configuration version changes when its target configuration changes, and reported telemetry records carry their applicable report time.
2. Dashboard error reports. Dashboard error reports are de-identified by design and do not include direct account identifiers, full URLs, query strings, form contents, or request or response bodies in the ordinary error-report flow. Messages and routes are normalized and identifiers, email addresses and token-shaped strings are replaced with placeholders before storage. Event-level error reports are retained for 90 days. Longer-lived error-pattern records retain a redacted message and code-location information rather than an account identifier. An identity is attached where the user affirmatively submits an identified problem report.
3. The contact form stores no IP address. Feedback submissions are emailed and not stored. Panic and error messages in the code never interpolate user or device data.
4. The Service applies request-size limits, ingest limits and route-specific rate controls appropriate to the applicable endpoint. Device-facing limits include controls on configuration polling and firmware downloads. Oversized bodies, over-cap telemetry batches and malformed frames may be refused.

### E. Availability and resilience

1. Managed PostgreSQL uses daily backups and continuous write-ahead-log archiving for point-in-time recovery through the database Sub-processor. Backups are retained and rotated according to that Sub-processor's normal backup schedule.
2. Per-device storage objects are durable and replicated by the edge provider; the relational mirror is a best-effort copy of them, so a mirror failure never loses device state.
3. Rate controls and ingest limits protect applicable device-facing routes from abuse and excessive traffic. A suspended account's devices may be refused at the ingest boundary in accordance with the Service's suspension controls.
4. The Provider maintains a public status page with automated probes of designated Service surfaces and a written incident-communication procedure with severity and communication guidance.

### F. Separation, logging and monitoring

1. Development, staging and production use separate configuration and secrets appropriate to each environment. Production and staging use logically separate databases and storage resources as described in Section C.4, although certain underlying infrastructure and identity services may be shared.
2. Edge invocation logs are retained by the edge provider for seven (7) days. Provider server logs are retained for 30 days, and the VPS maintains a system journal. New error signatures may generate operator alerts subject to rate controls. Failed device-authentication attempts are monitored and throttled by source address.
3. Designated Service surfaces are probed on a five-minute schedule. The Service also runs applicable retention sweeps on a five-minute schedule.

### G. Retention, deletion and return

1. **Automatic retention.** Telemetry history is retained according to the service tier at which the relevant account or organization is being served: 7 days while served at free-tier limits, 30 days on Builder, 90 days on Growth, and 13 months on Scale and Fleet. The applicable retention sweep runs every five minutes. Where telemetry is forwarded without stored history being created, no telemetry-history retention period applies.

Error-report events are retained for 90 days. Longer-lived error-pattern records retain de-identified or redacted error information as described in Section D.2. Device log buffers retain the newest 200 chunks per device. User sessions ordinarily last four hours.

Organization invitations are single-use and expire after seven days. Acceptance or expiry prevents further use but does not itself delete the invitation record; invitation records are removed on revocation or deletion of the applicable organization. Provider server logs are retained for 30 days. Firmware and related binaries are retained until the Customer requests removal or they are otherwise deleted under Section 8.

2. **Customer-driven deletion.** Deleting a device removes that device's stored state, including its access list, stored telemetry history, shadow/configuration state, device logs and device credentials, and closes active device connections as applicable. The Service also attempts removal of the corresponding relational-mirror record after the device deletion succeeds. Deleting a fleet that holds no devices removes that fleet. Deleting an organization that owns no fleets removes the organization and its associated membership and invitation records. A signed-in user may delete their own identified error reports through the available mechanism.
3. **Account deletion.** Account deletion is performed through the Provider's documented account-deletion procedure. The procedure removes or dissociates, as applicable, the Customer's devices, eligible fleets and organizations through the relevant deletion paths; removes the applicable identity record and active credentials and sessions; removes identified error reports subject to deletion; removes saved dashboard state and the applicable consent or acceptance record; and removes or dissociates other Customer-associated Service records that are subject to deletion under the Privacy Policy and Section 8.

Provider-controlled records that the Privacy Policy permits or requires the Provider to retain, including applicable billing, tax, legal or correspondence records, are not treated as Customer Personal Data solely because an account is deleted.

4. **Return.** Where return is requested under Section 8, the Provider will return reasonably available Customer Personal Data through documented API routes, machine-readable exports and, where necessary, other reasonable methods used to provide data not exposed through those routes.
5. **Backups.**  Database backups expire under the database Sub-processor's normal backup-rotation schedule and are not selectively purged. Customer Personal Data remaining only in backups remains subject to the confidentiality and security obligations of this DPA until the applicable backup expires.

### H. Sub-processor and infrastructure assurance

The Provider relies on appropriate contractual, technical and organizational controls of its infrastructure and Sub-processors in addition to the measures the Provider operates directly.

The current published Sub-processor list identifies the material infrastructure and service providers used by the Provider and includes, where applicable, the security certifications, assurance reports or other published security information on which the Provider relies. The Provider reviews available security and privacy documentation of material Sub-processors as appropriate to the services they provide.

Physical security of the data-center and hosting facilities used by the Service is provided by the applicable infrastructure Sub-processors. The Provider's own administrative access to those systems is controlled as described in Section A.

### I. Organizational measures

1. A single operator with production access; no support staff; the administrative console is a break-glass tool and its allow-list is not widened for support tasks.
2. Every production deployment, package publication and repository push is gated on the owner's explicit approval; changes are reviewed against the written architecture notes and shipped to staging first.
3. The platform's source is public, which is treated as a design constraint: every client-supplied field is validated as hostile, and no security control depends on secrecy of the code.
4. Data-protection and Data Subject requests are handled under the Provider's documented request-handling procedure and the responsibilities stated in Section 4.5.
5. Personal Data Breach handling follows the incident-communication procedure and the notification timeframe in Section 5.
6. Testing and evaluation. The Provider periodically reviews the effectiveness of the technical and organizational measures described in this Annex through operational monitoring, review of security-relevant changes and incidents, and testing appropriate to the size and risk profile of the Service. Material deficiencies identified through that process are tracked for remediation.

---

## Annex III: Sub-processors

The current list of Sub-processors engaged by the Provider, including each Sub-processor's legal entity, service provided, categories of data processed, processing location and applicable transfer mechanism, is published at:

[**https://pidgeiot.com/subprocessors/**](https://pidgeiot.com/subprocessors/)

The published list is maintained under Section 6 of this DPA and will identify its current version or effective date. Changes to the list are handled in accordance with Section 6.
