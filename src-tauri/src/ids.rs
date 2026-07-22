/// All internal ids are UUIDv7 strings (DEVELOPMENT.md §7): time-ordered, so
/// they sort by creation and index well as primary keys.
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_time_ordered() {
        let first = new_id();
        let second = new_id();

        assert_ne!(first, second);
        assert!(first < second, "v7 ids should sort by creation time");
        assert_eq!(first.len(), 36);
    }
}
