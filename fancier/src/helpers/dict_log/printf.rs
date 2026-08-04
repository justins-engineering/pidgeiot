//! C-style printf formatting for decoded cbprintf arguments.
//!
//! The Python reference hands the (lightly rewritten) format string to
//! Python's own `%` operator; this module implements the same observable
//! behavior directly for the conversions Zephyr logs can carry:
//! `d i u o x X c s p %` plus the float family `f F e E g G`, with the
//! standard flags (`- + space 0 #`), width, precision, and (ignored, since
//! argument sizing already happened at extraction time) length modifiers
//! `hh h l ll j z t L`.
//!
//! Reference quirks reproduced on purpose (parity over correctness):
//! - `%p` renders as `0x` + bare lowercase hex (the reference literally
//!   rewrites `%p` -> `0x%x` before formatting).
//! - a `*` width/precision does NOT consume an argument (the reference's
//!   extractor never extracts one), so it formats as if no width was given.

/// One extracted argument. Signedness/width decisions already happened at
/// extraction time (`decode.rs`), mirroring how the reference's
/// `struct.unpack` yields ready-to-format Python ints/floats/strs.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
  Int(i64),
  Uint(u64),
  Double(f64),
  Str(String),
}

#[derive(Default, Clone, Copy)]
struct Flags {
  minus: bool,
  plus: bool,
  space: bool,
  zero: bool,
  alt: bool,
}

/// Formats `fmt` with `args`, consuming one argument per conversion (except
/// `%%`). Missing/mismatched arguments render as `<?>` rather than failing
/// the whole line -- a partially readable log line beats none.
pub fn format_message(fmt: &str, args: &[Arg]) -> String {
  let chars: Vec<char> = fmt.chars().collect();
  let mut out = String::with_capacity(fmt.len() + 16);
  let mut arg_i = 0usize;
  let mut i = 0usize;

  while i < chars.len() {
    if chars[i] != '%' {
      out.push(chars[i]);
      i += 1;
      continue;
    }

    // Parse one %-spec.
    let mut j = i + 1;
    let mut flags = Flags::default();
    while j < chars.len() {
      match chars[j] {
        '-' => flags.minus = true,
        '+' => flags.plus = true,
        ' ' => flags.space = true,
        '0' => flags.zero = true,
        '#' => flags.alt = true,
        _ => break,
      }
      j += 1;
    }

    let mut width = 0usize;
    if j < chars.len() && chars[j] == '*' {
      // Reference quirk: no argument was extracted for '*', so there is no
      // width value to apply -- treat as width 0.
      j += 1;
    } else {
      while j < chars.len() && chars[j].is_ascii_digit() {
        width = width * 10 + (chars[j] as usize - '0' as usize);
        j += 1;
      }
    }

    let mut precision: Option<usize> = None;
    if j < chars.len() && chars[j] == '.' {
      j += 1;
      let mut p = 0usize;
      if j < chars.len() && chars[j] == '*' {
        j += 1; // same '*' quirk as width
      } else {
        while j < chars.len() && chars[j].is_ascii_digit() {
          p = p * 10 + (chars[j] as usize - '0' as usize);
          j += 1;
        }
      }
      precision = Some(p);
    }

    // Length modifiers: parsed and discarded (extraction already sized the
    // argument).
    while j < chars.len() && matches!(chars[j], 'h' | 'l' | 'j' | 'z' | 't' | 'L') {
      j += 1;
    }

    let Some(&conv) = chars.get(j) else {
      // Dangling '%' at end of string: emit verbatim.
      out.extend(&chars[i..]);
      break;
    };

    if conv == '%' {
      out.push('%');
      i = j + 1;
      continue;
    }

    let arg = args.get(arg_i);
    arg_i += 1;

    let piece = match conv {
      'd' | 'i' => arg.map(|a| fmt_signed(to_i64(a), flags, width, precision)),
      'u' => arg.map(|a| fmt_unsigned(to_u64(a), 10, false, flags, width, precision, "")),
      'o' => {
        let prefix = if flags.alt { "0" } else { "" };
        arg.map(|a| fmt_unsigned(to_u64(a), 8, false, flags, width, precision, prefix))
      }
      'x' => arg.map(|a| {
        let v = to_u64(a);
        let prefix = if flags.alt && v != 0 { "0x" } else { "" };
        fmt_unsigned(v, 16, false, flags, width, precision, prefix)
      }),
      'X' => arg.map(|a| {
        let v = to_u64(a);
        let prefix = if flags.alt && v != 0 { "0X" } else { "" };
        fmt_unsigned(v, 16, true, flags, width, precision, prefix)
      }),
      'c' => arg.map(|a| {
        let s = char::from_u32(to_u64(a) as u32)
          .map(|c| c.to_string())
          .unwrap_or_else(|| "?".to_string());
        pad(&s, flags, width)
      }),
      's' => arg.map(|a| {
        let s = match a {
          Arg::Str(s) => s.clone(),
          other => format!("{other:?}"),
        };
        let s = match precision {
          Some(p) => s.chars().take(p).collect(),
          None => s,
        };
        pad(&s, flags, width)
      }),
      'p' => arg.map(|a| pad(&format!("0x{:x}", to_u64(a)), flags, width)),
      'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
        arg.map(|a| fmt_float(to_f64(a), conv, flags, width, precision))
      }
      _ => {
        // Unknown conversion: emit the raw spec text and put the argument
        // back (nothing consumed it in C either).
        arg_i -= 1;
        out.extend(&chars[i..=j]);
        i = j + 1;
        continue;
      }
    };

    out.push_str(&piece.unwrap_or_else(|| pad("<?>", flags, width)));
    i = j + 1;
  }

  out
}

fn to_i64(a: &Arg) -> i64 {
  match a {
    Arg::Int(v) => *v,
    Arg::Uint(v) => *v as i64,
    Arg::Double(v) => *v as i64,
    Arg::Str(_) => 0,
  }
}

fn to_u64(a: &Arg) -> u64 {
  match a {
    Arg::Int(v) => *v as u64,
    Arg::Uint(v) => *v,
    Arg::Double(v) => *v as u64,
    Arg::Str(_) => 0,
  }
}

fn to_f64(a: &Arg) -> f64 {
  match a {
    Arg::Int(v) => *v as f64,
    Arg::Uint(v) => *v as f64,
    Arg::Double(v) => *v,
    Arg::Str(_) => 0.0,
  }
}

/// Width padding for already-rendered strings (`%s`/`%c`/`%p`): right-
/// justified unless '-'.
fn pad(s: &str, flags: Flags, width: usize) -> String {
  let len = s.chars().count();
  if len >= width {
    return s.to_string();
  }
  let fill = " ".repeat(width - len);
  if flags.minus {
    format!("{s}{fill}")
  } else {
    format!("{fill}{s}")
  }
}

/// Shared final assembly for numeric conversions: sign/prefix, zero or
/// space padding, width. `digits` excludes sign and prefix.
fn assemble_number(
  sign: &str,
  prefix: &str,
  digits: String,
  flags: Flags,
  width: usize,
  int_precision: Option<usize>,
) -> String {
  // Integer precision = minimum digit count, zero-padded; it also disables
  // the '0' flag (C semantics).
  let digits = match int_precision {
    Some(p) if digits.chars().count() < p => {
      format!("{}{}", "0".repeat(p - digits.chars().count()), digits)
    }
    _ => digits,
  };

  let body_len = sign.len() + prefix.len() + digits.chars().count();
  if body_len >= width {
    return format!("{sign}{prefix}{digits}");
  }
  let pad_n = width - body_len;
  if flags.minus {
    format!("{sign}{prefix}{digits}{}", " ".repeat(pad_n))
  } else if flags.zero && int_precision.is_none() {
    // Zero padding goes between sign/prefix and digits.
    format!("{sign}{prefix}{}{digits}", "0".repeat(pad_n))
  } else {
    format!("{}{sign}{prefix}{digits}", " ".repeat(pad_n))
  }
}

fn fmt_signed(v: i64, flags: Flags, width: usize, precision: Option<usize>) -> String {
  let sign = if v < 0 {
    "-"
  } else if flags.plus {
    "+"
  } else if flags.space {
    " "
  } else {
    ""
  };
  let digits = v.unsigned_abs().to_string();
  assemble_number(sign, "", digits, flags, width, precision)
}

fn fmt_unsigned(
  v: u64,
  base: u32,
  upper: bool,
  flags: Flags,
  width: usize,
  precision: Option<usize>,
  prefix: &str,
) -> String {
  let digits = match base {
    8 => format!("{v:o}"),
    16 if upper => format!("{v:X}"),
    16 => format!("{v:x}"),
    _ => v.to_string(),
  };
  // '#o' means "ensure a leading 0" -- skip the extra 0 if one's already
  // there (or precision will add zeros anyway).
  let (prefix, digits) =
    if prefix == "0" && (digits.starts_with('0') || precision.is_some_and(|p| p > digits.len())) {
      ("", digits)
    } else {
      (prefix, digits)
    };
  assemble_number("", prefix, digits, flags, width, precision)
}

fn fmt_float(v: f64, conv: char, flags: Flags, width: usize, precision: Option<usize>) -> String {
  let upper = conv.is_ascii_uppercase();
  let sign = if v.is_sign_negative() {
    "-"
  } else if flags.plus {
    "+"
  } else if flags.space {
    " "
  } else {
    ""
  };
  let av = v.abs();

  if av.is_nan() {
    return pad(
      &format!("{sign}{}", if upper { "NAN" } else { "nan" }),
      flags,
      width,
    );
  }
  if av.is_infinite() {
    return pad(
      &format!("{sign}{}", if upper { "INF" } else { "inf" }),
      flags,
      width,
    );
  }

  let body = match conv.to_ascii_lowercase() {
    'f' => fmt_fixed(av, precision.unwrap_or(6), flags.alt),
    'e' => fmt_exp(av, precision.unwrap_or(6), flags.alt),
    'g' => {
      let p = match precision.unwrap_or(6) {
        0 => 1,
        p => p,
      };
      // C %g: use %e when exponent < -4 or >= precision, else %f with
      // adjusted precision; strip trailing zeros unless '#'.
      let exp = decimal_exponent(av, p);
      if exp < -4 || exp >= p as i32 {
        let s = fmt_exp(av, p.saturating_sub(1), flags.alt);
        if flags.alt { s } else { strip_g_zeros_exp(&s) }
      } else {
        let prec = (p as i32 - 1 - exp).max(0) as usize;
        let s = fmt_fixed(av, prec, flags.alt);
        if flags.alt {
          s
        } else {
          strip_g_zeros_fixed(&s)
        }
      }
    }
    _ => unreachable!(),
  };
  let body = if upper { body.to_uppercase() } else { body };

  // Assemble with sign + optional zero padding.
  let body_len = sign.len() + body.chars().count();
  if body_len >= width {
    return format!("{sign}{body}");
  }
  let pad_n = width - body_len;
  if flags.minus {
    format!("{sign}{body}{}", " ".repeat(pad_n))
  } else if flags.zero {
    format!("{sign}{}{body}", "0".repeat(pad_n))
  } else {
    format!("{}{sign}{body}", " ".repeat(pad_n))
  }
}

fn fmt_fixed(av: f64, prec: usize, alt: bool) -> String {
  let s = format!("{av:.prec$}");
  if alt && prec == 0 { format!("{s}.") } else { s }
}

/// `%e` body: `d.dddddde±XX` with a minimum two-digit exponent.
fn fmt_exp(av: f64, prec: usize, alt: bool) -> String {
  if av == 0.0 {
    let mantissa = fmt_fixed(0.0, prec, alt);
    return format!("{mantissa}e+00");
  }
  let mut exp = av.log10().floor() as i32;
  let mut mantissa = av / 10f64.powi(exp);
  // log10/powi rounding can land the mantissa just outside [1, 10); nudge.
  if mantissa >= 10.0 {
    mantissa /= 10.0;
    exp += 1;
  } else if mantissa < 1.0 {
    mantissa *= 10.0;
    exp -= 1;
  }
  // Rounding the mantissa to `prec` digits can push it to 10.0 (e.g.
  // 9.999... at prec 2) -- renormalize after rounding.
  let mut rounded = format!("{mantissa:.prec$}");
  if rounded.starts_with("10") {
    exp += 1;
    rounded = format!("{:.prec$}", mantissa / 10.0);
  }
  let rounded = if alt && prec == 0 {
    format!("{rounded}.")
  } else {
    rounded
  };
  let (esign, eabs) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
  format!("{rounded}e{esign}{eabs:02}")
}

/// The power-of-ten exponent `%g` decides on, accounting for the fact that
/// rounding to `p` significant digits can bump it (0.9999 at p=2 -> 1.0).
fn decimal_exponent(av: f64, p: usize) -> i32 {
  if av == 0.0 {
    return 0;
  }
  let mut exp = av.log10().floor() as i32;
  let scaled = av / 10f64.powi(exp);
  if scaled >= 10.0 {
    exp += 1;
  } else if scaled < 1.0 {
    exp -= 1;
  }
  // Does rounding to p significant digits carry over into the next decade?
  let mantissa = av / 10f64.powi(exp);
  let rounded: f64 = format!("{mantissa:.*}", p.saturating_sub(1))
    .parse()
    .unwrap_or(mantissa);
  if rounded >= 10.0 { exp + 1 } else { exp }
}

fn strip_g_zeros_fixed(s: &str) -> String {
  if !s.contains('.') {
    return s.to_string();
  }
  let s = s.trim_end_matches('0');
  s.trim_end_matches('.').to_string()
}

fn strip_g_zeros_exp(s: &str) -> String {
  let Some(epos) = s.find('e') else {
    return s.to_string();
  };
  let (mantissa, exp) = s.split_at(epos);
  if !mantissa.contains('.') {
    return s.to_string();
  }
  let m = mantissa.trim_end_matches('0');
  let m = m.trim_end_matches('.');
  format!("{m}{exp}")
}

#[cfg(test)]
mod tests {
  use super::*;

  fn f(fmt: &str, args: &[Arg]) -> String {
    format_message(fmt, args)
  }

  #[test]
  fn plain_and_percent_literal() {
    assert_eq!(f("hello", &[]), "hello");
    assert_eq!(f("100%% done", &[]), "100% done");
    assert_eq!(f("%d%%", &[Arg::Int(5)]), "5%");
  }

  #[test]
  fn signed_integers() {
    assert_eq!(f("%d", &[Arg::Int(42)]), "42");
    assert_eq!(f("%d", &[Arg::Int(-42)]), "-42");
    assert_eq!(f("%i", &[Arg::Int(0)]), "0");
    assert_eq!(f("%5d", &[Arg::Int(42)]), "   42");
    assert_eq!(f("%-5d|", &[Arg::Int(42)]), "42   |");
    assert_eq!(f("%05d", &[Arg::Int(42)]), "00042");
    assert_eq!(f("%05d", &[Arg::Int(-42)]), "-0042");
    assert_eq!(f("%+d", &[Arg::Int(42)]), "+42");
    assert_eq!(f("% d", &[Arg::Int(42)]), " 42");
    assert_eq!(f("%.4d", &[Arg::Int(42)]), "0042");
    assert_eq!(f("%8.4d", &[Arg::Int(42)]), "    0042");
    // Length modifiers parsed + ignored.
    assert_eq!(
      f(
        "%ld %lld %hd %hhd",
        &[Arg::Int(1), Arg::Int(2), Arg::Int(3), Arg::Int(4)]
      ),
      "1 2 3 4"
    );
    assert_eq!(f("%d", &[Arg::Int(i64::MIN)]), "-9223372036854775808");
  }

  #[test]
  fn unsigned_and_bases() {
    assert_eq!(f("%u", &[Arg::Uint(42)]), "42");
    assert_eq!(f("%u", &[Arg::Uint(u64::MAX)]), "18446744073709551615");
    assert_eq!(f("%x", &[Arg::Uint(0xdead)]), "dead");
    assert_eq!(f("%X", &[Arg::Uint(0xdead)]), "DEAD");
    assert_eq!(f("%#x", &[Arg::Uint(0xdead)]), "0xdead");
    assert_eq!(f("%#X", &[Arg::Uint(0xdead)]), "0XDEAD");
    assert_eq!(f("%#x", &[Arg::Uint(0)]), "0"); // no 0x for zero, like C
    assert_eq!(f("%o", &[Arg::Uint(8)]), "10");
    assert_eq!(f("%#o", &[Arg::Uint(8)]), "010");
    assert_eq!(f("%#o", &[Arg::Uint(0)]), "0"); // already has a leading 0
    assert_eq!(f("%08x", &[Arg::Uint(0xbeef)]), "0000beef");
    assert_eq!(f("%#010x", &[Arg::Uint(0xbeef)]), "0x0000beef");
  }

  #[test]
  fn chars_strings_pointers() {
    assert_eq!(f("%c", &[Arg::Uint('!' as u64)]), "!");
    assert_eq!(f("%s", &[Arg::Str("abc".into())]), "abc");
    assert_eq!(f("%8s|", &[Arg::Str("abc".into())]), "     abc|");
    assert_eq!(f("%-8s|", &[Arg::Str("abc".into())]), "abc     |");
    assert_eq!(f("%.2s", &[Arg::Str("abc".into())]), "ab");
    // Reference rewrites %p -> 0x%x.
    assert_eq!(f("%p", &[Arg::Uint(0x3bb4)]), "0x3bb4");
  }

  #[test]
  fn floats_fixed() {
    assert_eq!(f("%f", &[Arg::Double(68.69)]), "68.690000");
    assert_eq!(f("%f", &[Arg::Double(-0.5)]), "-0.500000");
    assert_eq!(f("%.2f", &[Arg::Double(3.14159)]), "3.14");
    assert_eq!(f("%.0f", &[Arg::Double(2.5)]), "2"); // ties-to-even, like glibc
    assert_eq!(f("%8.2f", &[Arg::Double(3.14159)]), "    3.14");
    assert_eq!(f("%-8.2f|", &[Arg::Double(3.14159)]), "3.14    |");
    assert_eq!(f("%08.2f", &[Arg::Double(-3.14159)]), "-0003.14");
    assert_eq!(f("%+.1f", &[Arg::Double(1.0)]), "+1.0");
  }

  #[test]
  fn floats_exponent() {
    assert_eq!(f("%e", &[Arg::Double(68.69)]), "6.869000e+01");
    assert_eq!(f("%E", &[Arg::Double(68.69)]), "6.869000E+01");
    assert_eq!(f("%.2e", &[Arg::Double(0.001234)]), "1.23e-03");
    assert_eq!(f("%e", &[Arg::Double(0.0)]), "0.000000e+00");
    assert_eq!(f("%.1e", &[Arg::Double(9.99)]), "1.0e+01"); // round carries
    assert_eq!(f("%.0e", &[Arg::Double(5.0)]), "5e+00");
  }

  #[test]
  fn floats_general() {
    assert_eq!(f("%g", &[Arg::Double(0.0001)]), "0.0001");
    assert_eq!(f("%g", &[Arg::Double(0.00001)]), "1e-05");
    assert_eq!(f("%g", &[Arg::Double(123456.0)]), "123456");
    assert_eq!(f("%g", &[Arg::Double(1234567.0)]), "1.23457e+06");
    assert_eq!(f("%g", &[Arg::Double(100.0)]), "100");
    assert_eq!(f("%.3g", &[Arg::Double(3.14159)]), "3.14");
    assert_eq!(f("%G", &[Arg::Double(0.00001)]), "1E-05");
  }

  #[test]
  fn float_specials() {
    assert_eq!(f("%f", &[Arg::Double(f64::NAN)]), "nan");
    assert_eq!(f("%F", &[Arg::Double(f64::NAN)]), "NAN");
    assert_eq!(f("%f", &[Arg::Double(f64::INFINITY)]), "inf");
    assert_eq!(f("%f", &[Arg::Double(f64::NEG_INFINITY)]), "-inf");
  }

  #[test]
  fn graceful_on_missing_or_wrong_args() {
    assert_eq!(f("%d %d", &[Arg::Int(1)]), "1 <?>");
    assert_eq!(f("%s", &[]), "<?>");
    // Unknown conversion passes through without consuming an argument.
    assert_eq!(f("%q %d", &[Arg::Int(7)]), "%q 7");
    // Dangling percent at end.
    assert_eq!(f("50%", &[]), "50%");
  }

  #[test]
  fn star_width_is_inert() {
    // Reference-parser quirk: '*' never consumed an argument at extraction
    // time, so it must not consume one here either.
    assert_eq!(f("%*d", &[Arg::Int(42)]), "42");
    assert_eq!(f("%.*f", &[Arg::Double(1.5)]), "2"); // precision 0
  }
}
