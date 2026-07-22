use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// All persisted timestamps are UTC ISO 8601 (DEVELOPMENT.md §7).
pub fn now_iso8601() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("Rfc3339 formatting of a valid OffsetDateTime cannot fail")
}

/// A future UTC timestamp, used to gate a job's retry backoff.
pub fn plus_seconds(seconds: u64) -> String {
    (OffsetDateTime::now_utc() + time::Duration::seconds(seconds as i64))
        .format(&Rfc3339)
        .expect("Rfc3339 formatting of a valid OffsetDateTime cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backoff_deadline_lies_in_the_future() {
        let now = now_iso8601();
        let later = plus_seconds(120);

        assert!(later > now, "an ISO 8601 UTC string sorts lexicographically by time");
    }

    #[test]
    fn produces_a_parsable_utc_timestamp() {
        let now = now_iso8601();
        let parsed = OffsetDateTime::parse(&now, &Rfc3339).unwrap();
        assert_eq!(parsed.offset(), time::UtcOffset::UTC);
    }
}
