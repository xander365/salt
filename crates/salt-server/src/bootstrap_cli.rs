//! Argument parsing for `salt-server bootstrap` (issue #48). Kept as its own
//! step, the same way [`crate::config::ServerConfig::from_source`] is:
//! [`parse`] takes a plain argument list rather than reading `std::env`
//! itself, so a test drives it in-process with no real argv and no real
//! process. Everything this module decides is syntax — whether a flag is
//! given and whether its value has the right shape. Semantics — is this
//! `--period-end-day` actually 1..=28, does this Operator already exist —
//! stay in `payroll_app::bootstrap`, the one place they can be tested
//! without a process either.

use payroll_app::BootstrapPeriodEndDay;

/// `salt-server bootstrap`'s own arguments, parsed and shaped but not yet
/// validated against the database — `main` hands these straight to
/// [`payroll_app::bootstrap`], which is what actually creates anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapArgs {
    pub email: String,
    pub display_name: String,
    pub employer_name: String,
    pub period_end_day: BootstrapPeriodEndDay,
}

/// Why [`parse`] refused. Every variant carries enough to build a message
/// naming what was wrong and what a caller should pass instead — the same
/// discipline `payroll_app::PayrollAppError` applies to its own refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapArgsError {
    /// `flag` was never given at all.
    Missing(&'static str),
    /// `flag` was the last argument, with no value after it.
    MissingValue(&'static str),
    /// `arg` is not one of the four recognised flags.
    Unrecognized(String),
    /// `flag` was given more than once. Refused rather than resolved by a
    /// last-one-wins rule: bootstrap creates rows that cannot be created
    /// again, and a command line that names two Employers is a command line
    /// whose author does not agree with itself about which one to create.
    Repeated(&'static str),
    /// `--period-end-day`'s value is neither "last-day-of-month" nor
    /// something that parses as a plain integer. A value that parses but
    /// falls outside 1..=28 is not caught here — that is a domain rule of
    /// `payroll`'s own `DayOfMonth` type, checked once, inside
    /// `payroll_app::bootstrap`, rather than duplicated in this syntax-only
    /// layer.
    InvalidPeriodEndDay(String),
}

/// What every `--period-end-day` refusal explains, so a person who forgot
/// the flag or mistyped its value is told what to pass instead of only that
/// something was wrong (issue #48's own acceptance criterion: "refusing
/// without it prints what the options mean").
const PERIOD_END_DAY_HELP: &str = "--period-end-day must be a day of month from 1 to 28 (e.g. \
     --period-end-day 25), or the literal --period-end-day last-day-of-month. It cannot be \
     changed once payroll has been run, so it is never defaulted or guessed.";

impl std::fmt::Display for BootstrapArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing("--period-end-day") => {
                write!(f, "--period-end-day is required. {PERIOD_END_DAY_HELP}")
            }
            Self::Missing(flag) => write!(f, "{flag} is required"),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value"),
            Self::Unrecognized(arg) => write!(f, "unrecognized argument: {arg}"),
            Self::Repeated(flag) => write!(f, "{flag} was given more than once"),
            Self::InvalidPeriodEndDay(value) => {
                write!(
                    f,
                    "\"{value}\" is not a valid --period-end-day. {PERIOD_END_DAY_HELP}"
                )
            }
        }
    }
}

impl std::error::Error for BootstrapArgsError {}

/// Parses `args` — the words after `bootstrap` on the command line, e.g.
/// `["--email", "a@b.com", "--period-end-day", "25"]` — into
/// [`BootstrapArgs`]. Refuses as a whole on the first problem found, the
/// same discipline `ServerConfig::from_source` applies to its own
/// environment: a caller either has arguments it can bootstrap with, or one
/// error naming the first thing to fix.
pub fn parse(args: &[String]) -> Result<BootstrapArgs, BootstrapArgsError> {
    let mut email = None;
    let mut display_name = None;
    let mut employer_name = None;
    let mut period_end_day = None;

    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--email" => set_once(&mut email, next_value(&mut iter, "--email")?, "--email")?,
            "--display-name" => set_once(
                &mut display_name,
                next_value(&mut iter, "--display-name")?,
                "--display-name",
            )?,
            "--employer-name" => set_once(
                &mut employer_name,
                next_value(&mut iter, "--employer-name")?,
                "--employer-name",
            )?,
            "--period-end-day" => {
                let value = next_value(&mut iter, "--period-end-day")?;
                set_once(
                    &mut period_end_day,
                    parse_period_end_day(&value)?,
                    "--period-end-day",
                )?;
            }
            other => return Err(BootstrapArgsError::Unrecognized(other.to_string())),
        }
    }

    Ok(BootstrapArgs {
        email: email.ok_or(BootstrapArgsError::Missing("--email"))?,
        display_name: display_name.ok_or(BootstrapArgsError::Missing("--display-name"))?,
        employer_name: employer_name.ok_or(BootstrapArgsError::Missing("--employer-name"))?,
        period_end_day: period_end_day.ok_or(BootstrapArgsError::Missing("--period-end-day"))?,
    })
}

/// Stores `value` in `slot`, or refuses when `slot` already holds one — the
/// whole of the "a flag is given at most once" rule, in one place so no flag
/// can be the one that forgot it.
fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    flag: &'static str,
) -> Result<(), BootstrapArgsError> {
    if slot.is_some() {
        return Err(BootstrapArgsError::Repeated(flag));
    }
    *slot = Some(value);
    Ok(())
}

fn next_value(
    iter: &mut std::slice::Iter<'_, String>,
    flag: &'static str,
) -> Result<String, BootstrapArgsError> {
    iter.next()
        .cloned()
        .ok_or(BootstrapArgsError::MissingValue(flag))
}

fn parse_period_end_day(value: &str) -> Result<BootstrapPeriodEndDay, BootstrapArgsError> {
    if value == "last-day-of-month" {
        return Ok(BootstrapPeriodEndDay::LastDayOfMonth);
    }
    value
        .parse::<u8>()
        .map(BootstrapPeriodEndDay::Day)
        .map_err(|_| BootstrapArgsError::InvalidPeriodEndDay(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_complete_argument_list_parses_with_a_fixed_period_end_day() {
        let parsed = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--period-end-day",
            "25",
        ]))
        .unwrap();

        assert_eq!(
            parsed,
            BootstrapArgs {
                email: "alice@example.com".to_string(),
                display_name: "Alice".to_string(),
                employer_name: "Acme Corp".to_string(),
                period_end_day: BootstrapPeriodEndDay::Day(25),
            }
        );
    }

    #[test]
    fn last_day_of_month_is_recognised() {
        let parsed = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--period-end-day",
            "last-day-of-month",
        ]))
        .unwrap();

        assert_eq!(parsed.period_end_day, BootstrapPeriodEndDay::LastDayOfMonth);
    }

    #[test]
    fn flags_are_recognised_in_any_order() {
        let parsed = parse(&args(&[
            "--period-end-day",
            "25",
            "--employer-name",
            "Acme Corp",
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
        ]))
        .unwrap();

        assert_eq!(parsed.email, "alice@example.com");
    }

    #[test]
    fn a_missing_period_end_day_is_refused_and_explains_the_options() {
        let err = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
        ]))
        .expect_err("--period-end-day is required");

        assert_eq!(err, BootstrapArgsError::Missing("--period-end-day"));
        let message = err.to_string();
        assert!(message.contains("1 to 28"), "{message}");
        assert!(message.contains("last-day-of-month"), "{message}");
    }

    #[test]
    fn a_missing_email_is_refused() {
        let err = parse(&args(&[
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--period-end-day",
            "25",
        ]))
        .expect_err("--email is required");

        assert_eq!(err, BootstrapArgsError::Missing("--email"));
    }

    #[test]
    fn a_flag_with_no_value_is_refused() {
        let err = parse(&args(&["--email"])).expect_err("--email needs a value");

        assert_eq!(err, BootstrapArgsError::MissingValue("--email"));
    }

    #[test]
    fn a_repeated_flag_is_refused_rather_than_resolved_by_last_one_wins() {
        let err = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--employer-name",
            "Widgets Inc",
            "--period-end-day",
            "25",
        ]))
        .expect_err("--employer-name was given twice");

        assert_eq!(err, BootstrapArgsError::Repeated("--employer-name"));
    }

    #[test]
    fn a_repeated_period_end_day_is_refused() {
        let err = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--period-end-day",
            "25",
            "--period-end-day",
            "last-day-of-month",
        ]))
        .expect_err("--period-end-day was given twice");

        assert_eq!(err, BootstrapArgsError::Repeated("--period-end-day"));
    }

    #[test]
    fn an_unrecognized_flag_is_refused() {
        let err = parse(&args(&["--nickname", "Al"])).expect_err("--nickname is not recognised");

        assert_eq!(
            err,
            BootstrapArgsError::Unrecognized("--nickname".to_string())
        );
    }

    #[test]
    fn a_period_end_day_that_is_neither_a_number_nor_the_literal_is_refused() {
        let err = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--period-end-day",
            "banana",
        ]))
        .expect_err("\"banana\" is not a valid --period-end-day");

        assert_eq!(
            err,
            BootstrapArgsError::InvalidPeriodEndDay("banana".to_string())
        );
        let message = err.to_string();
        assert!(message.contains("1 to 28"), "{message}");
    }

    /// The CLI layer accepts any `u8` and leaves range-checking to
    /// `payroll_app::bootstrap` — 29 parses cleanly here even though it is
    /// not a valid day of month.
    #[test]
    fn a_syntactically_valid_but_out_of_range_day_parses_here_and_is_left_to_the_library() {
        let parsed = parse(&args(&[
            "--email",
            "alice@example.com",
            "--display-name",
            "Alice",
            "--employer-name",
            "Acme Corp",
            "--period-end-day",
            "29",
        ]))
        .unwrap();

        assert_eq!(parsed.period_end_day, BootstrapPeriodEndDay::Day(29));
    }
}
