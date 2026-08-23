//! `barista wake-at` — the two string conversions the wake alarm needs (nap-013).
//!
//! Contract A takes an **absolute** timestamp, because a deadline that has to
//! survive a node restart cannot be a countdown. A human rarely has one to hand,
//! so the CLI converts — and only here, so that everything else stays the thin
//! client nap-006 decided it should be.
//!
//! The accepted spellings are `docs/cli.md`'s (`2026-08-09T09:00:00Z`) plus a
//! relative duration, which is what an operator actually types when the answer is
//! "in five minutes".

use std::time::{SystemTime, UNIX_EPOCH};

/// Parse what a human typed into an absolute Unix time, in seconds.
///
/// - `2026-08-09T09:00:00Z`, or with a numeric offset (`…+02:00`) — the form
///   `docs/cli.md` publishes.
/// - `90s`, `5m`, `2h`, `3d`, optionally with a leading `+` — relative to `now`.
///
/// Named zones (`CEST`, `America/New_York`) are **refused** rather than guessed
/// at: resolving them needs a tz database this crate does not carry, and an alarm
/// armed an hour out is worse than one refused.
pub(crate) fn parse_when(when: &str, now: SystemTime) -> anyhow::Result<i64> {
    let when = when.trim();
    let body = when.strip_prefix('+').unwrap_or(when);
    anyhow::ensure!(!body.is_empty(), "wake-at needs a time; see --help");

    // A leading `+` promises a duration, so a timestamp is not looked for after
    // one — `+2026-…` is a typo, not a date.
    if when == body {
        if let Some(seconds) = parse_rfc3339(body)? {
            return Ok(seconds);
        }
    }

    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("this machine's clock is before the Unix epoch"))?
        .as_secs() as i64;

    // Split on the last *character*, not the last byte: `split_at` on a byte
    // index inside a multi-byte character panics, and a CLI has no business
    // panicking on whatever someone typed.
    let unit = body.chars().last();
    let digits = &body[..body.len() - unit.map_or(0, char::len_utf8)];
    // Both halves have to agree for this to be a duration. `9am` ends in a unit
    // and is still not one, and reporting it as a malformed *duration* would send
    // the reader looking for the wrong mistake.
    let relative = match unit {
        Some('s') => Some(1i64),
        Some('m') => Some(60),
        Some('h') => Some(3600),
        Some('d') => Some(86_400),
        _ => None,
    }
    .and_then(|multiplier| digits.parse::<i64>().ok().map(|n| (n, multiplier)));

    match relative {
        Some((n, multiplier)) => n
            .checked_mul(multiplier)
            .and_then(|d| now_secs.checked_add(d))
            .ok_or_else(|| anyhow::anyhow!("{when:?} is further away than time goes")),
        None => Err(anyhow::anyhow!(
            "{when:?} is neither a timestamp (2026-08-09T09:00:00Z) nor a duration \
             (90s, 5m, 2h, 3d)"
        )),
    }
}

/// `YYYY-MM-DDTHH:MM:SS` plus `Z` or `±HH:MM`, as seconds since the epoch.
///
/// `Ok(None)` means "this is not a timestamp at all, try the other spelling";
/// `Err` means "this was meant to be one and is wrong", which is the distinction
/// that lets the caller produce a useful message instead of a generic one.
///
/// Sub-second precision is accepted and discarded: the alarm's resolution is the
/// reconcile tick, so pretending to honour milliseconds would be a promise the
/// platform does not keep.
fn parse_rfc3339(s: &str) -> anyhow::Result<Option<i64>> {
    // The cheapest reliable discriminator, and it cannot collide with a duration:
    // no duration contains a `-`.
    if s.len() < 19 || !s.is_char_boundary(19) || s.as_bytes()[4] != b'-' {
        return Ok(None);
    }
    let (date_time, zone) = s.split_at(19);
    let number = |slice: &str, what: &str| -> anyhow::Result<i64> {
        slice
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("{s:?}: {what} is not a number"))
    };
    anyhow::ensure!(
        date_time.as_bytes()[7] == b'-'
            && (date_time.as_bytes()[10] == b'T' || date_time.as_bytes()[10] == b't')
            && date_time.as_bytes()[13] == b':'
            && date_time.as_bytes()[16] == b':',
        "{s:?} is not a timestamp: expected YYYY-MM-DDTHH:MM:SS with an offset, \
         e.g. 2026-08-09T09:00:00Z"
    );
    let (year, month, day) = (
        number(&date_time[0..4], "the year")?,
        number(&date_time[5..7], "the month")?,
        number(&date_time[8..10], "the day")?,
    );
    let (hour, minute, second) = (
        number(&date_time[11..13], "the hour")?,
        number(&date_time[14..16], "the minute")?,
        number(&date_time[17..19], "the second")?,
    );
    anyhow::ensure!(
        (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60,
        "{s:?} is not a real time"
    );
    // Day-against-month, so an impossible calendar date is refused rather than
    // guessed (the module's stated policy): without this, `days_from_civil`
    // silently rolls 2026-02-31 forward into March.
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    anyhow::ensure!(
        day <= days_in_month,
        "{s:?} is not a real date: month {month} has no day {day}"
    );

    // Fractional seconds, then the offset. Both optional in shape, but the offset
    // is not optional in meaning: a timestamp with no zone is ambiguous by
    // exactly the amount that decides whether an alarm fires at 9am or 11am.
    let zone = zone.strip_prefix('.').map_or(zone, |rest| {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        &rest[digits..]
    });
    let offset_secs = match zone {
        "Z" | "z" => 0,
        "" => anyhow::bail!(
            "{s:?} has no time zone, so it could mean several different moments; \
             write Z for UTC or an offset like +02:00"
        ),
        zone => {
            let sign = match zone.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => anyhow::bail!(
                    "{s:?}: {zone:?} is not an offset. Named zones need a tz database this \
                     tool does not carry — use Z or a numeric offset like +02:00"
                ),
            };
            let rest = zone[1..].replace(':', "");
            anyhow::ensure!(
                rest.len() == 4,
                "{s:?}: {zone:?} is not an offset like +02:00"
            );
            sign * (number(&rest[0..2], "the offset hours")? * 3600
                + number(&rest[2..4], "the offset minutes")? * 60)
        }
    };

    Ok(Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - offset_secs,
    ))
}

/// Days between 1970-01-01 and a proleptic-Gregorian date.
///
/// Howard Hinnant's `days_from_civil`, which is the standard closed form and is
/// here because the alternative is a date crate for one conversion. Correct for
/// every year the platform will see, leap years and centuries included.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // March-based years, so the leap day lands at the end and needs no special
    // case anywhere else.
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// How long until an absolute time, for a human reading a table.
///
/// Coarse on purpose — one unit, rounded down. Whoever is reading `barista ls` wants
/// to know whether an alarm is armed and roughly when, and a full timestamp in a
/// column would cost more width than that answer is worth. The `--json` form
/// keeps the exact value, which is where a script should be looking anyway.
pub(crate) fn until(target_secs: i64, now: SystemTime) -> String {
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = target_secs - now_secs;
    if delta <= 0 {
        // Armed and its moment has passed: the next reconcile tick will act on
        // it. Saying "due" rather than a negative countdown is the honest read.
        return "due".to_string();
    }
    match delta {
        d if d < 60 => format!("in {d}s"),
        d if d < 3600 => format!("in {}m", d / 60),
        d if d < 86_400 => format!("in {}h", d / 3600),
        d => format!("in {}d", d / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(epoch_secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(epoch_secs)
    }

    #[test]
    fn durations_are_relative_to_now() {
        let now = at(1_000_000);
        assert_eq!(parse_when("90s", now).unwrap(), 1_000_090);
        assert_eq!(parse_when("+5m", now).unwrap(), 1_000_300);
        assert_eq!(parse_when("2h", now).unwrap(), 1_007_200);
        assert_eq!(parse_when("1d", now).unwrap(), 1_086_400);
    }

    /// The form `docs/cli.md` publishes, and the leap-year arithmetic under it.
    #[test]
    fn the_documented_timestamp_form_is_the_moment_it_names() {
        let now = at(0);
        assert_eq!(parse_when("1970-01-01T00:00:00Z", now).unwrap(), 0);
        assert_eq!(
            parse_when("2026-08-09T09:00:00Z", now).unwrap(),
            1_786_266_000
        );
        // A leap day, and a century that is not a leap year, both on the path.
        assert_eq!(
            parse_when("2024-02-29T00:00:00Z", now).unwrap(),
            1_709_164_800
        );
        assert_eq!(
            parse_when("1900-03-01T00:00:00Z", now).unwrap(),
            -2_203_891_200
        );
        // Sub-second precision is accepted and dropped: the tick is the alarm's
        // real resolution, and pretending otherwise would be a promise unkept.
        assert_eq!(
            parse_when("2026-08-09T09:00:00.750Z", now).unwrap(),
            parse_when("2026-08-09T09:00:00Z", now).unwrap()
        );
    }

    /// An offset moves the moment, in the direction that is easy to get backwards:
    /// 09:00+02:00 is *earlier* in absolute terms than 09:00Z.
    #[test]
    fn a_numeric_offset_shifts_the_moment_the_right_way() {
        let now = at(0);
        let utc = parse_when("2026-08-09T09:00:00Z", now).unwrap();
        assert_eq!(
            parse_when("2026-08-09T09:00:00+02:00", now).unwrap(),
            utc - 7200
        );
        assert_eq!(
            parse_when("2026-08-09T09:00:00-05:00", now).unwrap(),
            utc + 18000
        );
    }

    /// The refusals that matter, each because guessing would arm the alarm at the
    /// wrong time rather than fail.
    #[test]
    fn ambiguous_or_malformed_times_are_refused_with_something_to_do_about_it() {
        let now = at(1_000_000);

        let no_zone = parse_when("2026-08-09T09:00:00", now)
            .unwrap_err()
            .to_string();
        assert!(
            no_zone.contains("time zone"),
            "a zoneless timestamp must say what is missing: {no_zone}"
        );

        let named = parse_when("2026-08-09T09:00:00CEST", now)
            .unwrap_err()
            .to_string();
        assert!(
            named.contains("Named zones"),
            "a named zone must be refused rather than guessed: {named}"
        );

        let nonsense = parse_when("9am", now).unwrap_err().to_string();
        assert!(
            nonsense.contains("2026-08-09T09:00:00Z"),
            "the error must show what to type: {nonsense}"
        );

        assert!(parse_when("2026-13-09T09:00:00Z", now).is_err(), "month 13");
        // Day-against-month: an impossible calendar date is refused, not rolled
        // forward. 2026 is not a leap year, so Feb has 28 days.
        assert!(parse_when("2026-02-31T09:00:00Z", now).is_err(), "Feb 31");
        assert!(
            parse_when("2026-02-29T09:00:00Z", now).is_err(),
            "Feb 29 (non-leap)"
        );
        assert!(parse_when("2026-04-31T09:00:00Z", now).is_err(), "Apr 31");
        assert!(
            parse_when("2024-02-29T09:00:00Z", now).is_ok(),
            "Feb 29 (leap)"
        );
        // A `+` promises a duration, so the unit is not optional after it.
        assert!(parse_when("+300", now).is_err());
        assert!(parse_when("", now).is_err());
    }

    #[test]
    fn a_countdown_reads_as_one_unit_and_says_when_it_is_due() {
        let now = at(1_000_000);
        assert_eq!(until(1_000_030, now), "in 30s");
        assert_eq!(until(1_000_300, now), "in 5m");
        assert_eq!(until(1_010_000, now), "in 2h");
        assert_eq!(until(1_200_000, now), "in 2d");
        assert_eq!(until(999_999, now), "due");
    }
}
