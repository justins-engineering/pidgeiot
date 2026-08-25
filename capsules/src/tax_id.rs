//! Tax identity for the billing entity: what kind of tax identifier an
//! organization filed, what we currently believe about it, and the format
//! rules that decide whether it is even worth asking VIES.
//!
//! Validation lives in the shared crate for the same reason the contact
//! form's does: the dashboard field and dovecote's route must agree on
//! what a well-formed identifier is, and calling one function is the only
//! way that stays true.
//!
//! Two things this module deliberately does NOT do. It does not compute
//! national check digits -- VIES already rejects a checksum-failing number
//! locally, without contacting the member state (observed: a
//! checksum-broken German number answers `valid: false` in the same second
//! that checksum-valid German numbers answer `MS_UNAVAILABLE`), so a second
//! implementation here would only be a second thing to get wrong. And it
//! does not decide whether a number is *registered* -- that is VIES's
//! answer alone, and the whole point of [`TaxIdStatus::Pending`] is that we
//! are honest when we could not get it.

use serde::{Deserialize, Serialize};

/// Longest identifier we store, after normalization. Comfortably above the
/// 14 characters the longest EU VAT number occupies, with room for the
/// non-EU registrations `TaxIdType::Other` exists to hold.
pub const MAX_TAX_ID_CHARS: usize = 32;

/// Longest legal/business name we store. Long enough for a real registered
/// name with its suffixes, short enough that the column is not a free-text
/// dumping ground.
pub const MAX_BUSINESS_NAME_CHARS: usize = 200;

/// What kind of tax identifier an organization filed.
///
/// `EuVat` is the only variant we check against an authority ourselves.
/// The jurisdiction-specific variants exist because a registration is only
/// worth anything to the billing provider once it knows which authority
/// issued it: Stripe's tax-ID enum is per jurisdiction, and a business tax
/// ID it can place is what lets it zero-rate or reverse-charge a sale to a
/// business abroad instead of adding tax to it. Their wire names are
/// Stripe's own, so the mapping is the name. `Other` holds a registration
/// from anywhere else: printed on the invoice, never forwarded.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaxIdType {
  /// No tax identifier on file. The resting state, and what clearing the
  /// field means.
  #[default]
  None,
  /// An EU (or Northern Ireland) VAT registration number, checkable
  /// against VIES.
  EuVat,
  /// United Kingdom VAT number (`GB...`).
  GbVat,
  /// Australian Business Number.
  AuAbn,
  /// Canadian GST/HST registration (`...RT0001`).
  CaGstHst,
  /// Canadian Business Number.
  CaBn,
  /// Indian GST registration.
  InGst,
  /// United States Employer Identification Number.
  UsEin,
  /// New Zealand GST number.
  NzGst,
  /// Singapore GST number.
  SgGst,
  /// Japanese Tax Registration Number (`T...`).
  JpTrn,
  /// Norwegian VAT number (`...MVA`).
  NoVat,
  /// South African VAT number.
  ZaVat,
  /// Any other jurisdiction's registration. Stored as given,
  /// format-sanity-checked, never checked remotely, never forwarded.
  Other,
}

impl TaxIdType {
  /// Every variant, in the order a picker should offer them.
  pub const ALL: &'static [TaxIdType] = &[
    TaxIdType::None,
    TaxIdType::EuVat,
    TaxIdType::GbVat,
    TaxIdType::AuAbn,
    TaxIdType::CaGstHst,
    TaxIdType::CaBn,
    TaxIdType::InGst,
    TaxIdType::UsEin,
    TaxIdType::NzGst,
    TaxIdType::SgGst,
    TaxIdType::JpTrn,
    TaxIdType::NoVat,
    TaxIdType::ZaVat,
    TaxIdType::Other,
  ];

  pub fn as_str(&self) -> &'static str {
    match self {
      TaxIdType::None => "none",
      TaxIdType::EuVat => "eu_vat",
      TaxIdType::GbVat => "gb_vat",
      TaxIdType::AuAbn => "au_abn",
      TaxIdType::CaGstHst => "ca_gst_hst",
      TaxIdType::CaBn => "ca_bn",
      TaxIdType::InGst => "in_gst",
      TaxIdType::UsEin => "us_ein",
      TaxIdType::NzGst => "nz_gst",
      TaxIdType::SgGst => "sg_gst",
      TaxIdType::JpTrn => "jp_trn",
      TaxIdType::NoVat => "no_vat",
      TaxIdType::ZaVat => "za_vat",
      TaxIdType::Other => "other",
    }
  }

  /// What a person sees in a picker.
  pub fn label(&self) -> &'static str {
    match self {
      TaxIdType::None => "None",
      TaxIdType::EuVat => "EU VAT",
      TaxIdType::GbVat => "UK VAT",
      TaxIdType::AuAbn => "Australian ABN",
      TaxIdType::CaGstHst => "Canadian GST/HST",
      TaxIdType::CaBn => "Canadian BN",
      TaxIdType::InGst => "Indian GST",
      TaxIdType::UsEin => "US EIN",
      TaxIdType::NzGst => "New Zealand GST",
      TaxIdType::SgGst => "Singapore GST",
      TaxIdType::JpTrn => "Japan tax registration",
      TaxIdType::NoVat => "Norwegian VAT",
      TaxIdType::ZaVat => "South African VAT",
      TaxIdType::Other => "Other",
    }
  }

  /// The Stripe tax-ID `type` this registration is forwarded as, or `None`
  /// for the two kinds that cannot be: nothing on file, and a registration
  /// whose jurisdiction Stripe's enum cannot name.
  pub fn stripe_type(&self) -> Option<&'static str> {
    match self {
      TaxIdType::None | TaxIdType::Other => None,
      forwardable => Some(forwardable.as_str()),
    }
  }

  /// Whether we ask an authority about this kind ourselves.
  pub fn is_checked_remotely(&self) -> bool {
    matches!(self, TaxIdType::EuVat)
  }
}

impl std::str::FromStr for TaxIdType {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    TaxIdType::ALL
      .iter()
      .copied()
      .find(|kind| kind.as_str() == s)
      .ok_or_else(|| format!("invalid tax id type '{s}'"))
  }
}

impl std::fmt::Display for TaxIdType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// What we currently believe about the stored identifier.
///
/// The states are deliberately separate rather than a `bool` plus a
/// timestamp, because "we asked and it is good", "we asked and it is not",
/// "we could not ask" and "we do not check this kind" are four different
/// things to tell a customer, and collapsing any two of them into one
/// would be a claim we cannot support.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaxIdStatus {
  /// Nothing on file.
  #[default]
  None,
  /// An EU VAT number we hold but could not get a definitive answer for --
  /// VIES was unreachable, the member state was down, or the request timed
  /// out. Retried by the scheduled sweep. This is the state that makes the
  /// "a VIES outage never blocks a save" rule expressible.
  Pending,
  /// VIES confirmed the number is a live registration.
  Validated,
  /// VIES said the number is not a registration. Only reachable through a
  /// re-check of a row that already exists: at save time a definitive
  /// invalid refuses the write outright, so nothing lands in this state by
  /// being entered.
  Invalid,
  /// Held but not checked, because we do not check this kind. The honest
  /// label for a non-EU registration.
  Unverified,
}

impl TaxIdStatus {
  pub fn as_str(&self) -> &'static str {
    match self {
      TaxIdStatus::None => "none",
      TaxIdStatus::Pending => "pending",
      TaxIdStatus::Validated => "validated",
      TaxIdStatus::Invalid => "invalid",
      TaxIdStatus::Unverified => "unverified",
    }
  }

  /// Whether this status still owes us a VIES answer. The scheduled sweep
  /// selects on exactly this, narrowed to `TaxIdType::EuVat` -- nothing
  /// else has an authority to ask.
  pub fn awaits_lookup(&self) -> bool {
    matches!(self, TaxIdStatus::Pending)
  }
}

impl std::str::FromStr for TaxIdStatus {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "none" => Ok(TaxIdStatus::None),
      "pending" => Ok(TaxIdStatus::Pending),
      "validated" => Ok(TaxIdStatus::Validated),
      "invalid" => Ok(TaxIdStatus::Invalid),
      "unverified" => Ok(TaxIdStatus::Unverified),
      other => Err(format!("invalid tax id status '{other}'")),
    }
  }
}

impl std::fmt::Display for TaxIdStatus {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

/// What a VIES lookup concluded, reduced to the three answers that change
/// what we store. `Unknown` covers every way of not getting an answer --
/// transport failure, a member state reporting itself unavailable, a
/// timeout, an unparseable body -- because they all mean the same thing to
/// us and none of them is evidence about the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViesOutcome {
  Valid,
  Invalid,
  Unknown,
}

/// What to do with a tax id the caller is trying to save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaxIdDecision {
  /// Write the identifier with this status.
  Store(TaxIdStatus),
  /// Refuse the write. Reached only on a definitive VIES `invalid`.
  Refuse,
}

/// The save-time half of the state machine.
///
/// `lookup` is `None` when no lookup was attempted (a non-EU identifier,
/// or a cleared field). The asymmetry with [`recheck_status`] is the point:
/// only here can a definitive invalid refuse, because only here is there
/// no stored row yet to leave behind.
pub fn decide_status(kind: TaxIdType, lookup: Option<ViesOutcome>) -> TaxIdDecision {
  match kind {
    TaxIdType::None => TaxIdDecision::Store(TaxIdStatus::None),
    TaxIdType::EuVat => match lookup {
      Some(ViesOutcome::Valid) => TaxIdDecision::Store(TaxIdStatus::Validated),
      Some(ViesOutcome::Invalid) => TaxIdDecision::Refuse,
      // An outage is not an answer, so it cannot be a refusal. The number
      // is kept and the sweep asks again.
      Some(ViesOutcome::Unknown) | None => TaxIdDecision::Store(TaxIdStatus::Pending),
    },
    // Nothing else has an authority we ask; held as declared.
    _ => TaxIdDecision::Store(TaxIdStatus::Unverified),
  }
}

/// The sweep-time half: a row already exists, so there is nothing to
/// refuse. A number VIES now calls invalid becomes visibly invalid rather
/// than disappearing.
pub fn recheck_status(outcome: ViesOutcome) -> TaxIdStatus {
  match outcome {
    ViesOutcome::Valid => TaxIdStatus::Validated,
    ViesOutcome::Invalid => TaxIdStatus::Invalid,
    ViesOutcome::Unknown => TaxIdStatus::Pending,
  }
}

/// Why a submitted identifier was rejected before anyone was asked about
/// it. Carries no fragment of the identifier itself, so a rejection can be
/// logged as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaxIdFormatError {
  /// A type was chosen but no identifier was supplied.
  Missing,
  /// An identifier was supplied alongside `TaxIdType::None`, which is the
  /// instruction to clear the field. Refusing beats guessing which the
  /// caller meant.
  UnexpectedForNone,
  TooLong,
  /// The identifier does not begin with a country code VIES serves.
  UnknownCountry,
  /// Right country, wrong shape for that country's numbering.
  CountryShape(&'static str),
  /// A non-EU identifier that is not plausibly an identifier at all.
  Implausible,
}

impl std::fmt::Display for TaxIdFormatError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      TaxIdFormatError::Missing => f.write_str("a tax ID is required for the selected type"),
      TaxIdFormatError::UnexpectedForNone => f.write_str(
        "no tax ID can be stored when the type is 'none' -- clear the ID or choose a type",
      ),
      TaxIdFormatError::TooLong => write!(
        f,
        "tax ID is longer than {MAX_TAX_ID_CHARS} characters after normalization"
      ),
      TaxIdFormatError::UnknownCountry => f.write_str(
        "an EU VAT ID must begin with the two-letter country code of an EU member state (or XI for Northern Ireland)",
      ),
      TaxIdFormatError::CountryShape(country) => {
        write!(f, "that is not the shape of a {country} VAT number")
      }
      TaxIdFormatError::Implausible => {
        f.write_str("a tax ID must be 4 to 32 letters and digits")
      }
    }
  }
}

/// A well-formed EU VAT identifier, split the way VIES wants it: the
/// member-state code and the national number separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EuVatId {
  pub country: String,
  pub number: String,
}

impl EuVatId {
  /// The identifier as stored and displayed -- country code and number,
  /// no separator.
  pub fn full(&self) -> String {
    format!("{}{}", self.country, self.number)
  }
}

/// Strips the punctuation people paste along with an identifier and
/// upper-cases what is left. Whitespace, dots, dashes and slashes are the
/// separators printed on real invoices; nothing else is removed, so a
/// genuinely odd character still reaches the shape check and is rejected
/// there rather than silently vanishing.
///
/// Public because it is also the equality the billing provider's copy of
/// a number is compared under: Stripe stores what its own form was given,
/// separators and all.
pub fn normalize_tax_id(raw: &str) -> String {
  normalize(raw)
}

fn normalize(raw: &str) -> String {
  raw
    .chars()
    .filter(|c| !c.is_whitespace() && !matches!(c, '.' | '-' | '/'))
    .flat_map(|c| c.to_uppercase())
    .collect()
}

/// Per-country shape patterns. Lowercase letters are character classes --
/// `d` a digit, `a` a letter, `n` either -- and every uppercase character
/// is a literal, which is unambiguous because a normalized identifier is
/// already upper-case.
///
/// These are typo catchers, not validators: their job is to spend a VIES
/// call only on something that could plausibly be an answer, and to give
/// the person typing an immediate, specific reason when it could not.
/// Greece files VAT under `EL` while its ISO code is `GR`; `GR` is accepted
/// as an alias and stored as `EL`, which is the form VIES answers to.
fn shapes_for(country: &str) -> Option<(&'static str, &'static [&'static str])> {
  Some(match country {
    "AT" => ("AT", &["Udddddddd"]),
    "BE" => ("BE", &["ddddddddd", "dddddddddd"]),
    "BG" => ("BG", &["ddddddddd", "dddddddddd"]),
    "CY" => ("CY", &["dddddddda"]),
    "CZ" => ("CZ", &["dddddddd", "ddddddddd", "dddddddddd"]),
    "DE" => ("DE", &["ddddddddd"]),
    "DK" => ("DK", &["dddddddd"]),
    "EE" => ("EE", &["ddddddddd"]),
    "EL" => ("EL", &["ddddddddd"]),
    "ES" => ("ES", &["ndddddddn"]),
    "FI" => ("FI", &["dddddddd"]),
    "FR" => ("FR", &["nnddddddddd"]),
    "HR" => ("HR", &["ddddddddddd"]),
    "HU" => ("HU", &["dddddddd"]),
    "IE" => ("IE", &["ddddddda", "dddddddaa", "daddddda"]),
    "IT" => ("IT", &["ddddddddddd"]),
    "LT" => ("LT", &["ddddddddd", "dddddddddddd"]),
    "LU" => ("LU", &["dddddddd"]),
    "LV" => ("LV", &["ddddddddddd"]),
    "MT" => ("MT", &["dddddddd"]),
    "NL" => ("NL", &["dddddddddBdd"]),
    "PL" => ("PL", &["dddddddddd"]),
    "PT" => ("PT", &["ddddddddd"]),
    // Romania's number is genuinely variable-length, from two digits up.
    "RO" => (
      "RO",
      &[
        "dd",
        "ddd",
        "dddd",
        "ddddd",
        "dddddd",
        "ddddddd",
        "dddddddd",
        "ddddddddd",
        "dddddddddd",
      ],
    ),
    "SE" => ("SE", &["dddddddddddd"]),
    "SI" => ("SI", &["dddddddd"]),
    "SK" => ("SK", &["dddddddddd"]),
    // Northern Ireland keeps the UK numbering under the Windsor Framework,
    // including the government-department and health-authority forms.
    "XI" => ("XI", &["ddddddddd", "dddddddddddd", "GDddd", "HAddd"]),
    _ => return None,
  })
}

fn matches_shape(number: &str, shape: &str) -> bool {
  if number.chars().count() != shape.chars().count() {
    return false;
  }
  number.chars().zip(shape.chars()).all(|(c, s)| match s {
    'd' => c.is_ascii_digit(),
    'a' => c.is_ascii_uppercase(),
    'n' => c.is_ascii_alphanumeric(),
    literal => c == literal,
  })
}

/// Parses and shape-checks an EU VAT identifier. Accepts the number with
/// or without its country prefix repeated, in any case, with the
/// separators people actually type.
pub fn parse_eu_vat(raw: &str) -> Result<EuVatId, TaxIdFormatError> {
  let normalized = normalize(raw);
  if normalized.is_empty() {
    return Err(TaxIdFormatError::Missing);
  }
  if normalized.chars().count() > MAX_TAX_ID_CHARS {
    return Err(TaxIdFormatError::TooLong);
  }
  if normalized.chars().count() < 3 {
    return Err(TaxIdFormatError::UnknownCountry);
  }

  let prefix: String = normalized.chars().take(2).collect();
  let lookup_code = if prefix == "GR" {
    "EL"
  } else {
    prefix.as_str()
  };
  let Some((country, shapes)) = shapes_for(lookup_code) else {
    return Err(TaxIdFormatError::UnknownCountry);
  };

  let number: String = normalized.chars().skip(2).collect();
  if shapes.iter().any(|shape| matches_shape(&number, shape)) {
    Ok(EuVatId {
      country: country.to_string(),
      number,
    })
  } else {
    Err(TaxIdFormatError::CountryShape(country))
  }
}

/// Format sanity for an identifier we will never check remotely: it has to
/// look like an identifier, and that is the whole of the claim we make
/// about it.
pub fn normalize_other_tax_id(raw: &str) -> Result<String, TaxIdFormatError> {
  let normalized = normalize(raw);
  if normalized.is_empty() {
    return Err(TaxIdFormatError::Missing);
  }
  if normalized.chars().count() > MAX_TAX_ID_CHARS {
    return Err(TaxIdFormatError::TooLong);
  }
  if normalized.chars().count() < 4 || !normalized.chars().all(|c| c.is_ascii_alphanumeric()) {
    return Err(TaxIdFormatError::Implausible);
  }
  Ok(normalized)
}

/// The stored form of a submitted identifier, plus the VIES coordinates if
/// one is owed. `lookup` is `Some` for exactly the identifiers we are
/// going to ask about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTaxId {
  pub stored: Option<String>,
  pub lookup: Option<EuVatId>,
}

/// The single entry point a route uses: turns a submitted `(type, id)`
/// pair into what should be stored and what, if anything, still has to be
/// asked of VIES.
pub fn prepare_tax_id(
  kind: TaxIdType,
  raw: Option<&str>,
) -> Result<PreparedTaxId, TaxIdFormatError> {
  let raw = raw.map(str::trim).filter(|s| !s.is_empty());
  match kind {
    TaxIdType::None => match raw {
      Some(_) => Err(TaxIdFormatError::UnexpectedForNone),
      None => Ok(PreparedTaxId {
        stored: None,
        lookup: None,
      }),
    },
    TaxIdType::EuVat => {
      let Some(raw) = raw else {
        return Err(TaxIdFormatError::Missing);
      };
      let parsed = parse_eu_vat(raw)?;
      Ok(PreparedTaxId {
        stored: Some(parsed.full()),
        lookup: Some(parsed),
      })
    }
    _ => {
      let Some(raw) = raw else {
        return Err(TaxIdFormatError::Missing);
      };
      Ok(PreparedTaxId {
        stored: Some(normalize_other_tax_id(raw)?),
        lookup: None,
      })
    }
  }
}

/// How a tax identifier is named in a log line: its kind, its country
/// prefix where it has one, and its length. Never the identifier.
///
/// A VAT number is not a secret -- it is printed on every invoice its
/// owner issues -- but it identifies a customer, and logs are the one place
/// customer identifiers accumulate without anyone deciding they should. The
/// prefix and the length are what an operator actually needs to read a
/// failure.
pub fn tax_id_log_label(kind: TaxIdType, stored: &str) -> String {
  let len = stored.chars().count();
  match kind {
    TaxIdType::EuVat => {
      let prefix: String = stored.chars().take(2).collect();
      format!("eu_vat {prefix}/{len}")
    }
    TaxIdType::None => "none".to_string(),
    other => format!("{other} len {len}"),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn accepts_real_vat_numbers_across_the_shape_table() {
    // One per distinct shape family, so a botched pattern shows up as a
    // rejection rather than as a VIES call that was never worth making.
    for raw in [
      "ATU12345678",
      "BE0123456789",
      "BG123456789",
      "CY12345678L",
      "CZ12345678",
      "DE129273398",
      "DK12345678",
      "EE123456789",
      "EL123456789",
      "ESA1234567L",
      "FI12345678",
      "FR12345678901",
      "HR12345678901",
      "HU12345678",
      "IE6388047V",
      "IE1234567FA",
      "IT12345678901",
      "LT123456789",
      "LU12345678",
      "LV40003032949",
      "MT12345678",
      "NL810462783B01",
      "PL1234567890",
      "PT123456789",
      "RO123",
      "SE123456789012",
      "SI12345678",
      "SK1234567890",
      "XI123456789",
      "XIGD001",
    ] {
      assert!(
        parse_eu_vat(raw).is_ok(),
        "expected {raw} to parse as a well-formed VAT id"
      );
    }
  }

  #[test]
  fn rejects_wrong_shapes_for_the_right_country() {
    // Germany is nine digits; eight and ten are the two typos that
    // actually happen, and neither should cost a VIES call.
    assert_eq!(
      parse_eu_vat("DE12927339"),
      Err(TaxIdFormatError::CountryShape("DE"))
    );
    assert_eq!(
      parse_eu_vat("DE1292733989"),
      Err(TaxIdFormatError::CountryShape("DE"))
    );
    // Austria's number carries a literal U that is easy to drop.
    assert_eq!(
      parse_eu_vat("AT12345678"),
      Err(TaxIdFormatError::CountryShape("AT"))
    );
    // The Netherlands' literal B sits in a fixed position.
    assert_eq!(
      parse_eu_vat("NL810462783A01"),
      Err(TaxIdFormatError::CountryShape("NL"))
    );
    // A letter where Ireland wants a digit.
    assert_eq!(
      parse_eu_vat("IE638804AV"),
      Err(TaxIdFormatError::CountryShape("IE"))
    );
  }

  #[test]
  fn rejects_countries_vies_does_not_serve() {
    // The UK left VIES; only Northern Ireland's XI prefix remains.
    assert_eq!(
      parse_eu_vat("GB123456789"),
      Err(TaxIdFormatError::UnknownCountry)
    );
    assert_eq!(
      parse_eu_vat("US123456789"),
      Err(TaxIdFormatError::UnknownCountry)
    );
    assert_eq!(
      parse_eu_vat("12345678"),
      Err(TaxIdFormatError::UnknownCountry)
    );
  }

  #[test]
  fn greece_is_accepted_under_either_code_and_stored_as_el() {
    let parsed = parse_eu_vat("GR123456789").expect("GR should be accepted as an alias for EL");
    assert_eq!(parsed.country, "EL");
    assert_eq!(parsed.full(), "EL123456789");
  }

  #[test]
  fn normalization_survives_the_punctuation_people_paste() {
    // All four of these are the same registration copied off an invoice.
    for raw in [
      " ie 6388047v ",
      "IE-6388047-V",
      "IE.6388047.V",
      "ie/6388047/v",
    ] {
      let parsed = parse_eu_vat(raw).expect("a punctuated id should normalize");
      assert_eq!(parsed.full(), "IE6388047V");
    }
  }

  #[test]
  fn oversized_input_is_refused_before_anything_else() {
    let long = format!("IE{}", "9".repeat(MAX_TAX_ID_CHARS));
    assert_eq!(parse_eu_vat(&long), Err(TaxIdFormatError::TooLong));
    assert_eq!(
      normalize_other_tax_id(&"A".repeat(MAX_TAX_ID_CHARS + 1)),
      Err(TaxIdFormatError::TooLong)
    );
  }

  #[test]
  fn non_eu_ids_get_sanity_only() {
    assert_eq!(
      normalize_other_tax_id(" 51 824 753 556 ").unwrap(),
      "51824753556"
    );
    assert_eq!(normalize_other_tax_id("12-3456789").unwrap(), "123456789");
    assert_eq!(
      normalize_other_tax_id("abc"),
      Err(TaxIdFormatError::Implausible)
    );
    assert_eq!(
      normalize_other_tax_id("no spaces? ok!"),
      Err(TaxIdFormatError::Implausible)
    );
    assert_eq!(
      normalize_other_tax_id("   "),
      Err(TaxIdFormatError::Missing)
    );
  }

  #[test]
  fn prepare_routes_each_type_to_the_right_work() {
    let eu = prepare_tax_id(TaxIdType::EuVat, Some("ie6388047v")).unwrap();
    assert_eq!(eu.stored.as_deref(), Some("IE6388047V"));
    assert_eq!(
      eu.lookup,
      Some(EuVatId {
        country: "IE".into(),
        number: "6388047V".into()
      })
    );

    // A non-EU id is stored and never looked up.
    let other = prepare_tax_id(TaxIdType::Other, Some("51 824 753 556")).unwrap();
    assert_eq!(other.stored.as_deref(), Some("51824753556"));
    assert_eq!(other.lookup, None);

    // Clearing is `none` with nothing supplied.
    let cleared = prepare_tax_id(TaxIdType::None, None).unwrap();
    assert_eq!(cleared.stored, None);
    assert_eq!(cleared.lookup, None);

    // An id alongside `none` is ambiguous, so it is refused rather than
    // half-honoured.
    assert_eq!(
      prepare_tax_id(TaxIdType::None, Some("IE6388047V")),
      Err(TaxIdFormatError::UnexpectedForNone)
    );
    // A type without an id is the other half of the same ambiguity.
    assert_eq!(
      prepare_tax_id(TaxIdType::EuVat, Some("   ")),
      Err(TaxIdFormatError::Missing)
    );
    assert_eq!(
      prepare_tax_id(TaxIdType::Other, None),
      Err(TaxIdFormatError::Missing)
    );
  }

  #[test]
  fn an_outage_stores_pending_and_never_refuses() {
    // The rule the whole feature turns on: VIES being down must cost the
    // customer nothing.
    assert_eq!(
      decide_status(TaxIdType::EuVat, Some(ViesOutcome::Unknown)),
      TaxIdDecision::Store(TaxIdStatus::Pending)
    );
    assert_eq!(
      decide_status(TaxIdType::EuVat, None),
      TaxIdDecision::Store(TaxIdStatus::Pending)
    );
  }

  #[test]
  fn only_a_definitive_invalid_refuses_a_save() {
    assert_eq!(
      decide_status(TaxIdType::EuVat, Some(ViesOutcome::Invalid)),
      TaxIdDecision::Refuse
    );
    assert_eq!(
      decide_status(TaxIdType::EuVat, Some(ViesOutcome::Valid)),
      TaxIdDecision::Store(TaxIdStatus::Validated)
    );
    // Neither other type can refuse, whatever a stray lookup said.
    for lookup in [
      None,
      Some(ViesOutcome::Valid),
      Some(ViesOutcome::Invalid),
      Some(ViesOutcome::Unknown),
    ] {
      assert_eq!(
        decide_status(TaxIdType::Other, lookup),
        TaxIdDecision::Store(TaxIdStatus::Unverified)
      );
      assert_eq!(
        decide_status(TaxIdType::None, lookup),
        TaxIdDecision::Store(TaxIdStatus::None)
      );
    }
  }

  #[test]
  fn a_recheck_can_land_on_invalid_where_a_save_could_not() {
    // This is the only path into `Invalid`: the row already exists, so
    // there is nothing left to refuse.
    assert_eq!(recheck_status(ViesOutcome::Invalid), TaxIdStatus::Invalid);
    assert_eq!(recheck_status(ViesOutcome::Valid), TaxIdStatus::Validated);
    // Still no answer: stay pending, get swept again.
    assert_eq!(recheck_status(ViesOutcome::Unknown), TaxIdStatus::Pending);
  }

  #[test]
  fn only_pending_is_swept() {
    assert!(TaxIdStatus::Pending.awaits_lookup());
    for settled in [
      TaxIdStatus::None,
      TaxIdStatus::Validated,
      TaxIdStatus::Invalid,
      TaxIdStatus::Unverified,
    ] {
      assert!(!settled.awaits_lookup(), "{settled} should not be swept");
    }
  }

  #[test]
  fn log_labels_carry_no_identifier() {
    assert_eq!(
      tax_id_log_label(TaxIdType::EuVat, "IE6388047V"),
      "eu_vat IE/10"
    );
    assert_eq!(
      tax_id_log_label(TaxIdType::Other, "51824753556"),
      "other len 11"
    );
    assert_eq!(tax_id_log_label(TaxIdType::None, ""), "none");
    // The digits themselves must not survive into the label.
    let label = tax_id_log_label(TaxIdType::EuVat, "IE6388047V");
    assert!(!label.contains("6388047"), "log label leaked the number");
  }

  #[test]
  fn every_type_forwards_under_its_own_wire_name_or_is_explicitly_unforwardable() {
    for kind in TaxIdType::ALL {
      match kind.stripe_type() {
        // The wire name IS Stripe's enum value; anything else here would
        // be a second spelling to keep in step.
        Some(stripe) => assert_eq!(stripe, kind.as_str(), "{kind}"),
        None => assert!(
          matches!(kind, TaxIdType::None | TaxIdType::Other),
          "{kind} has no Stripe type but is not one of the two kinds that may lack one"
        ),
      }
    }
    assert!(TaxIdType::EuVat.is_checked_remotely());
    assert!(
      TaxIdType::ALL
        .iter()
        .filter(|k| k.is_checked_remotely())
        .count()
        == 1
    );
  }

  #[test]
  fn an_unknown_type_fails_to_parse_and_serde_refuses_it_too() {
    assert!("xx_vat".parse::<TaxIdType>().is_err());
    assert!("".parse::<TaxIdType>().is_err());
    assert!("EU_VAT".parse::<TaxIdType>().is_err());
    assert!(serde_json::from_str::<TaxIdType>("\"xx_vat\"").is_err());
    assert_eq!(
      serde_json::from_str::<TaxIdType>("\"ca_gst_hst\"").unwrap(),
      TaxIdType::CaGstHst
    );
  }

  #[test]
  fn jurisdiction_types_are_held_as_declared_and_never_looked_up() {
    let gb = prepare_tax_id(TaxIdType::GbVat, Some("GB 123 4567 89")).unwrap();
    assert_eq!(gb.stored.as_deref(), Some("GB123456789"));
    assert_eq!(gb.lookup, None);
    assert_eq!(
      decide_status(TaxIdType::GbVat, None),
      TaxIdDecision::Store(TaxIdStatus::Unverified)
    );
    assert_eq!(
      decide_status(TaxIdType::AuAbn, Some(ViesOutcome::Invalid)),
      TaxIdDecision::Store(TaxIdStatus::Unverified)
    );
    assert_eq!(
      tax_id_log_label(TaxIdType::GbVat, "GB123456789"),
      "gb_vat len 11"
    );
    assert_eq!(normalize_tax_id(" de-123.456/789 "), "DE123456789");
  }

  #[test]
  fn wire_forms_round_trip_and_stored_forms_parse() {
    assert_eq!(
      serde_json::to_string(&TaxIdType::EuVat).unwrap(),
      "\"eu_vat\""
    );
    assert_eq!(
      serde_json::to_string(&TaxIdType::CaGstHst).unwrap(),
      "\"ca_gst_hst\""
    );
    assert_eq!(
      serde_json::to_string(&TaxIdStatus::Unverified).unwrap(),
      "\"unverified\""
    );
    for kind in TaxIdType::ALL {
      assert_eq!(kind.as_str().parse::<TaxIdType>().unwrap(), *kind);
      let json = serde_json::to_string(kind).unwrap();
      assert_eq!(json, format!("\"{}\"", kind.as_str()));
      assert_eq!(serde_json::from_str::<TaxIdType>(&json).unwrap(), *kind);
    }
    for status in [
      TaxIdStatus::None,
      TaxIdStatus::Pending,
      TaxIdStatus::Validated,
      TaxIdStatus::Invalid,
      TaxIdStatus::Unverified,
    ] {
      assert_eq!(status.as_str().parse::<TaxIdStatus>().unwrap(), status);
    }
  }
}
